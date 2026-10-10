//! Cost events, pricing and the `/api/costs` summary.

use std::collections::BTreeMap;

use jiff::tz::TimeZone;
use omni_api::costs::{
    CostCategory, CostDailyRow, CostDayTotal, CostFeatureRow, CostPriceStatus, CostRange,
    CostRecentEvent, CostServiceRow, CostUsage, CostUsageTotals, CostsRangeInfo, CostsResponse,
    CostsSummary,
};
use omni_core::clock::SharedClock;
use omni_store::DocOps;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

use crate::Usage;

const LOG: &str = "Costs";
const DAY_MS: i64 = 86_400_000;

/// `cost-event`, keyed by `eventId`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEventData {
    pub category: CostCategory,
    pub feature: String,
    pub operation: String,
    pub service: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `number | null`: always written, `null` when unpriced.
    pub cost_cents: Option<f64>,
    pub price_status: CostPriceStatus,
    pub usage: CostUsage,
    pub event_id: String,
    pub incurred_at: i64,
    /// The task run that incurred the cost; absent outside a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for CostEventData {
    const NAME: &'static str = "cost-event";
    type Key = String;
    fn key(&self) -> String {
        self.event_id.clone()
    }
}

/// Id, time and run default to a new UUID, now and
/// the current run.
#[derive(Clone, Debug, PartialEq)]
pub struct NewCostEvent {
    pub category: CostCategory,
    pub feature: String,
    pub operation: String,
    pub service: String,
    pub model: Option<String>,
    pub cost_cents: Option<f64>,
    pub price_status: CostPriceStatus,
    pub usage: CostUsage,
    pub event_id: Option<String>,
    pub incurred_at: Option<i64>,
    pub run_id: Option<String>,
}

impl NewCostEvent {
    /// Fills id, time and the current run.
    pub fn into_event(self, now_ms: i64) -> CostEventData {
        CostEventData {
            category: self.category,
            feature: self.feature,
            operation: self.operation,
            service: self.service,
            model: self.model,
            cost_cents: self.cost_cents,
            price_status: self.price_status,
            usage: self.usage,
            event_id: self.event_id.unwrap_or_else(omni_core::ids::uuid_v4),
            incurred_at: self.incurred_at.unwrap_or(now_ms),
            run_id: self
                .run_id
                .or_else(|| omni_tasks::current_run().map(|run| run.run_id)),
            extra: Extra::new(),
        }
    }
}

/// Persists cost events; never fails the caller.
#[derive(Clone)]
pub struct CostRecorder {
    store: Store,
    clock: SharedClock,
}

impl CostRecorder {
    pub fn new(store: Store, clock: SharedClock) -> Self {
        Self { store, clock }
    }

    /// A failed write is logged, never returned, so
    /// telemetry cannot turn a paid provider call into a retry.
    pub async fn record(&self, e: NewCostEvent) {
        let event = e.into_event(self.clock.now_ms());
        let result = self
            .store
            .write(move |tx| tx.upsert(&event, UpsertOpts::default()))
            .await;
        if let Err(error) = result {
            tracing::error!(target: LOG, %error, "Failed to persist cost event");
        }
    }
}

/// USD cents per token for one model (`MODEL_PRICES`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrice {
    pub input_cents_per_token: f64,
    pub output_cents_per_token: f64,
}

/// Keyed by the bare model id.
pub const MODEL_PRICES: &[(&str, ModelPrice)] = &[
    (
        "gemini-3-flash-preview",
        ModelPrice {
            input_cents_per_token: 0.00005,
            output_cents_per_token: 0.0003,
        },
    ),
    (
        "gemini-3.5-flash",
        ModelPrice {
            input_cents_per_token: 0.00015,
            output_cents_per_token: 0.0009,
        },
    ),
    (
        "gemini-3.1-flash-lite",
        ModelPrice {
            input_cents_per_token: 0.00003,
            output_cents_per_token: 0.00025,
        },
    ),
    (
        "gemini-3.1-flash",
        ModelPrice {
            input_cents_per_token: 0.000015,
            output_cents_per_token: 0.00006,
        },
    ),
    (
        "gemini-3.5-flash-lite",
        ModelPrice {
            input_cents_per_token: 0.00003,
            output_cents_per_token: 0.00025,
        },
    ),
    (
        "gpt-6-astra",
        ModelPrice {
            input_cents_per_token: 0.001,
            output_cents_per_token: 0.005,
        },
    ),
    (
        "gpt-6-sol",
        ModelPrice {
            input_cents_per_token: 0.0002,
            output_cents_per_token: 0.001,
        },
    ),
    (
        "gpt-6-luna",
        ModelPrice {
            input_cents_per_token: 0.00001,
            output_cents_per_token: 0.00005,
        },
    ),
];

/// USD cents per synthesized character; zero means known self-hosted/free.
pub const TTS_CHARACTER_CENTS: &[(&str, f64)] = &[
    ("eleven_v3", 0.01),
    ("bosonai/higgs-audio-v3-tts-4b", 0.0),
    ("voxtral-mini-tts-2603", 0.0016),
];

/// The part after the last `:`.
pub fn bare_model_id(model: &str) -> &str {
    model.rsplit(':').next().unwrap_or(model)
}

fn price_of(model: &str) -> Option<ModelPrice> {
    let bare = bare_model_id(model);
    MODEL_PRICES
        .iter()
        .find(|(id, _)| *id == bare)
        .map(|(_, price)| *price)
}

/// Cost of one LLM call in USD cents; `None` when the model has no price
/// (recorded as `costCents: null`, `priceStatus: "unknown"`).
pub fn llm_cost_cents(model: &str, u: &Usage) -> Option<f64> {
    let price = price_of(model)?;
    #[allow(clippy::cast_precision_loss)]
    let cents = price.input_cents_per_token * u.input_tokens as f64
        + price.output_cents_per_token * u.output_tokens as f64;
    Some(cents)
}

/// Attribute by the current run's task name, else `fallback`.
pub fn current_cost_feature(fallback: &'static str) -> &'static str {
    let Some(run) = omni_tasks::current_run() else {
        return fallback;
    };
    let name = run.task_name.to_lowercase();
    if name.contains("presspods") {
        "press-pods"
    } else if name.contains("podcast") {
        "podcast-recommendations"
    } else if name.contains("recommendation") || name.contains("taste") {
        "media-recommendations"
    } else if name.contains("parcel") {
        "parcel-tracker"
    } else if name.contains("calendar") {
        "calendar-events"
    } else {
        fallback
    }
}

/// `cost-migration`, keyed by `version`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostMigrationData {
    pub version: String,
    pub completed_at: i64,
    pub imported_events: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for CostMigrationData {
    const NAME: &'static str = "cost-migration";
    type Key = String;
    fn key(&self) -> String {
        self.version.clone()
    }
}

/// The one historical import version.
pub const HISTORICAL_IMPORT_VERSION: &str = "historical-v1";

/// Read-only view of `press-pods-episode` rows (owned by `omni-presspods`) for the import.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPressPodsEpisode {
    episode_id: String,
    created_at: f64,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    voice_provider: Option<String>,
    #[serde(default)]
    costs: Option<LegacyEpisodeCosts>,
    #[serde(flatten)]
    extra: Extra,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyEpisodeCosts {
    llm_cents: f64,
    tts_cents: f64,
    #[serde(default)]
    detail_tokens: indexmap::IndexMap<String, LegacyTokens>,
    #[serde(default)]
    detail_chars: indexmap::IndexMap<String, f64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct LegacyTokens {
    input: f64,
    output: f64,
}

impl Entity for LegacyPressPodsEpisode {
    const NAME: &'static str = "press-pods-episode";
    type Key = String;
    fn key(&self) -> String {
        self.episode_id.clone()
    }
}

#[allow(clippy::cast_possible_truncation)]
fn ms(value: f64) -> i64 {
    value as i64
}

/// Seeds the ledger once from PressPods episodes that predate automatic capture. Returns the number of events
/// imported (0 when `historical-v1` already ran). Runs in one transaction.
pub async fn import_historical_costs(store: &Store) -> Result<u64, StoreError> {
    store
        .write(|tx| {
            if tx.has::<CostMigrationData>(&HISTORICAL_IMPORT_VERSION.to_owned())? {
                return Ok(0);
            }
            let now = tx.now_ms();
            let mut events = Vec::new();
            for episode in tx.get_all::<LegacyPressPodsEpisode>()? {
                let Some(costs) = &episode.costs else {
                    continue;
                };
                let (input, output) = costs
                    .detail_tokens
                    .values()
                    .fold((0.0, 0.0), |(i, o), t| (i + t.input, o + t.output));
                let characters: f64 = costs.detail_chars.values().sum();
                let llm_known = costs.detail_tokens.keys().all(|key| {
                    let model = key
                        .strip_suffix("-meta")
                        .or_else(|| key.strip_suffix("-clean"))
                        .unwrap_or(key);
                    price_of(model).is_some()
                });
                let tts_known = costs.detail_chars.keys().all(|key| {
                    let model = key.strip_suffix("-tts").unwrap_or(key);
                    TTS_CHARACTER_CENTS.iter().any(|(id, _)| *id == model)
                });
                events.push(NewCostEvent {
                    event_id: Some(format!("legacy:press-pods:llm:{}", episode.episode_id)),
                    incurred_at: Some(ms(episode.created_at)),
                    category: CostCategory::Llm,
                    feature: "press-pods".to_owned(),
                    operation: "historical-episode".to_owned(),
                    service: "legacy".to_owned(),
                    model: None,
                    cost_cents: llm_known.then_some(costs.llm_cents),
                    price_status: if llm_known {
                        CostPriceStatus::Estimated
                    } else {
                        CostPriceStatus::Unknown
                    },
                    usage: CostUsage {
                        input_tokens: Some(input),
                        output_tokens: Some(output),
                        ..CostUsage::default()
                    },
                    run_id: episode.run_id.clone(),
                });
                events.push(NewCostEvent {
                    event_id: Some(format!("legacy:press-pods:tts:{}", episode.episode_id)),
                    incurred_at: Some(ms(episode.created_at)),
                    category: CostCategory::Tts,
                    feature: "press-pods".to_owned(),
                    operation: "historical-episode".to_owned(),
                    service: episode
                        .voice_provider
                        .as_deref()
                        .map_or_else(|| "legacy".to_owned(), str::to_lowercase),
                    model: None,
                    cost_cents: tts_known.then_some(costs.tts_cents),
                    price_status: if !tts_known {
                        CostPriceStatus::Unknown
                    } else if costs.tts_cents == 0.0 {
                        CostPriceStatus::Free
                    } else {
                        CostPriceStatus::Estimated
                    },
                    usage: CostUsage {
                        characters: Some(characters),
                        ..CostUsage::default()
                    },
                    run_id: episode.run_id.clone(),
                });
            }
            let imported = events.len() as u64;
            for event in events {
                tx.upsert(&event.into_event(now), UpsertOpts::default())?;
            }
            tx.upsert(
                &CostMigrationData {
                    version: HISTORICAL_IMPORT_VERSION.to_owned(),
                    completed_at: now,
                    imported_events: imported,
                    extra: Extra::new(),
                },
                UpsertOpts::default(),
            )?;
            Ok::<_, StoreError>(imported)
        })
        .await
}

fn add_usage(target: &mut CostUsageTotals, usage: &CostUsage) {
    let add = |total: &mut f64, value: Option<f64>| *total += value.unwrap_or(0.0);
    add(&mut target.input_tokens, usage.input_tokens);
    add(
        &mut target.input_no_cache_tokens,
        usage.input_no_cache_tokens,
    );
    add(&mut target.cache_read_tokens, usage.cache_read_tokens);
    add(&mut target.cache_write_tokens, usage.cache_write_tokens);
    add(&mut target.output_tokens, usage.output_tokens);
    add(&mut target.reasoning_tokens, usage.reasoning_tokens);
    add(&mut target.characters, usage.characters);
    add(&mut target.requests, usage.requests);
    add(&mut target.credits, usage.credits);
}

fn day_key(ms: i64, tz: &TimeZone) -> String {
    omni_core::clock::timestamp_from_ms(ms)
        .to_zoned(tz.clone())
        .strftime("%Y-%m-%d")
        .to_string()
}

/// The cost summary for `GET /api/costs`.
pub fn summarize(
    events: &[CostEventData],
    days: CostRange,
    now: i64,
    tz: &TimeZone,
) -> CostsResponse {
    let from = days.days().map(|d| now - i64::from(d) * DAY_MS);
    let selected: Vec<&CostEventData> = events
        .iter()
        .filter(|e| e.incurred_at <= now && from.is_none_or(|from| e.incurred_at >= from))
        .collect();
    let all_time_cost_cents = events
        .iter()
        .filter(|e| e.incurred_at <= now)
        .map(|e| e.cost_cents.unwrap_or(0.0))
        .sum();
    let all_time_unknown_event_count = events
        .iter()
        .filter(|e| e.incurred_at <= now && e.cost_cents.is_none())
        .count() as u64;

    let mut usage = CostUsageTotals::default();
    let mut selected_cost_cents = 0.0;
    let mut unknown_event_count = 0u64;
    let mut daily: Vec<CostDailyRow> = Vec::new();
    let mut by_feature: Vec<CostFeatureRow> = Vec::new();
    let mut by_service: Vec<(String, CostServiceRow)> = Vec::new();

    for event in &selected {
        let cents = event.cost_cents.unwrap_or(0.0);
        let unknown = u64::from(event.cost_cents.is_none());
        selected_cost_cents += cents;
        add_usage(&mut usage, &event.usage);
        unknown_event_count += unknown;

        let date = day_key(event.incurred_at, tz);
        let index = match daily.iter().position(|d| d.date == date) {
            Some(index) => index,
            None => {
                daily.push(CostDailyRow {
                    date,
                    cost_cents: 0.0,
                    by_feature: BTreeMap::new(),
                    priced_event_count: 0,
                    unknown_event_count: 0,
                });
                daily.len() - 1
            }
        };
        let day = &mut daily[index];
        day.cost_cents += cents;
        *day.by_feature.entry(event.feature.clone()).or_insert(0.0) += cents;
        if event.cost_cents.is_some() {
            day.priced_event_count += 1;
        }
        day.unknown_event_count += unknown;

        let index = match by_feature.iter().position(|f| f.feature == event.feature) {
            Some(index) => index,
            None => {
                by_feature.push(CostFeatureRow {
                    feature: event.feature.clone(),
                    cost_cents: 0.0,
                    event_count: 0,
                    unknown_event_count: 0,
                });
                by_feature.len() - 1
            }
        };
        let feature = &mut by_feature[index];
        feature.cost_cents += cents;
        feature.event_count += 1;
        feature.unknown_event_count += unknown;

        let category = serde_json::to_value(event.category)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let key = format!(
            "{category}\0{}\0{}",
            event.service,
            event.model.as_deref().unwrap_or("")
        );
        let index = match by_service.iter().position(|(k, _)| *k == key) {
            Some(index) => index,
            None => {
                by_service.push((
                    key,
                    CostServiceRow {
                        service: event.service.clone(),
                        model: event.model.clone(),
                        category: event.category,
                        cost_cents: 0.0,
                        event_count: 0,
                        unknown_event_count: 0,
                        usage: CostUsageTotals::default(),
                    },
                ));
                by_service.len() - 1
            }
        };
        let service = &mut by_service[index].1;
        service.cost_cents += cents;
        service.event_count += 1;
        service.unknown_event_count += unknown;
        add_usage(&mut service.usage, &event.usage);
    }

    daily.sort_by(|a, b| omni_core::js::locale_compare(&a.date, &b.date));
    let highest_day = daily
        .iter()
        .filter(|day| day.priced_event_count > 0)
        .fold(None::<&CostDailyRow>, |highest, day| match highest {
            Some(h) if day.cost_cents <= h.cost_cents => Some(h),
            _ => Some(day),
        })
        .map(|day| CostDayTotal {
            date: day.date.clone(),
            cost_cents: day.cost_cents,
        });
    #[allow(clippy::cast_precision_loss)]
    let elapsed_days: f64 = match days.days() {
        Some(days) => f64::from(days),
        None => match selected.iter().map(|e| e.incurred_at).min() {
            None => 0.0,
            Some(earliest) => ((now - earliest) as f64 / DAY_MS as f64).ceil().max(1.0),
        },
    };

    by_feature.sort_by(|a, b| b.cost_cents.total_cmp(&a.cost_cents));
    let mut by_service: Vec<CostServiceRow> = by_service.into_iter().map(|(_, row)| row).collect();
    by_service.sort_by(|a, b| b.cost_cents.total_cmp(&a.cost_cents));
    let mut recent: Vec<&CostEventData> = selected.clone();
    recent.sort_by_key(|e| std::cmp::Reverse(e.incurred_at));
    let recent = recent
        .into_iter()
        .take(50)
        .map(|e| CostRecentEvent {
            event_id: e.event_id.clone(),
            incurred_at: e.incurred_at,
            category: e.category,
            feature: e.feature.clone(),
            operation: e.operation.clone(),
            service: e.service.clone(),
            model: e.model.clone(),
            cost_cents: e.cost_cents,
            price_status: e.price_status,
            usage: e.usage.clone(),
            run_id: e.run_id.clone(),
        })
        .collect();

    CostsResponse {
        range: CostsRangeInfo {
            days: days.days(),
            from,
            to: now,
        },
        summary: CostsSummary {
            selected_cost_cents,
            all_time_cost_cents,
            all_time_unknown_event_count,
            average_daily_cost_cents: if elapsed_days == 0.0 {
                0.0
            } else {
                selected_cost_cents / elapsed_days
            },
            highest_day,
            event_count: selected.len() as u64,
            unknown_event_count,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            characters: usage.characters,
            requests: usage.requests,
            credits: usage.credits,
        },
        daily,
        by_feature,
        by_service,
        recent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, at: i64, feature: &str, cents: Option<f64>) -> CostEventData {
        CostEventData {
            event_id: id.to_owned(),
            incurred_at: at,
            category: CostCategory::Llm,
            feature: feature.to_owned(),
            operation: "op".to_owned(),
            service: "openai".to_owned(),
            model: Some("gpt-6-luna".to_owned()),
            cost_cents: cents,
            price_status: if cents.is_some() {
                CostPriceStatus::Estimated
            } else {
                CostPriceStatus::Unknown
            },
            usage: CostUsage {
                input_tokens: Some(10.0),
                ..CostUsage::default()
            },
            run_id: None,
            extra: Extra::new(),
        }
    }

    #[test]
    fn prices_known_models_only() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Usage::default()
        };
        assert_eq!(
            llm_cost_cents("openai:gpt-6-luna", &usage).map(|c| c.round()),
            Some(60.0)
        );
        assert_eq!(llm_cost_cents("openai:unknown", &usage), None);
    }

    #[test]
    fn summarizes_by_day_feature_and_service() {
        let tz = TimeZone::UTC;
        let now = 10 * DAY_MS;
        let events = vec![
            event("a", now - DAY_MS, "briefings", Some(2.0)),
            event("b", now - 2 * DAY_MS, "workspaces", Some(5.0)),
            event("c", now - DAY_MS, "briefings", None),
            event("old", now - 40 * DAY_MS, "briefings", Some(100.0)),
        ];
        let summary = summarize(&events, CostRange::Days(30), now, &tz);
        assert_eq!(summary.summary.selected_cost_cents, 7.0);
        assert_eq!(summary.summary.all_time_cost_cents, 107.0);
        assert_eq!(summary.summary.unknown_event_count, 1);
        assert_eq!(summary.daily.len(), 2);
        assert_eq!(summary.daily[0].date, "1970-01-09");
        assert_eq!(summary.by_feature[0].feature, "workspaces");
        assert_eq!(summary.by_service.len(), 1);
        assert_eq!(summary.by_service[0].usage.input_tokens, 30.0);
        assert_eq!(
            summary.summary.highest_day,
            Some(CostDayTotal {
                date: "1970-01-09".to_owned(),
                cost_cents: 5.0
            })
        );
        assert_eq!(
            summary.recent.first().map(|e| e.event_id.as_str()),
            Some("a")
        );
    }
}
