//! `pets_read` and `costs_read`.

pub mod defs;

use jiff::tz::TimeZone;
use omni_ai::costs::{CostEventData, summarize};
use omni_api::costs::CostRange;
use omni_core::clock::SharedClock;
use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolMetaError, paginate, typed_tool};
use omni_store::{EntityOps, Store};
use serde::{Deserialize, Serialize};

use crate::pets::alerts::HealthLedger;
use crate::pets::persistence::{DailyVisitCount, PetStore, WeightHistoryRow};
use crate::pets::routes::health_response;

const MAX_MCP_COST_EVENTS: u64 = 100_000;

#[derive(Deserialize)]
#[serde(tag = "resource", rename_all = "lowercase")]
enum PetsReadInput {
    List {
        #[serde(default = "default_history_limit", rename = "historyLimit")]
        history_limit: usize,
    },
    History {
        #[serde(rename = "petId")]
        pet_id: String,
        #[serde(default)]
        cursor: usize,
        #[serde(default = "default_page_limit")]
        limit: usize,
    },
    Trend {
        #[serde(default, rename = "petId")]
        pet_id: Option<String>,
        #[serde(default = "default_trend_weeks")]
        weeks: u32,
    },
}

fn default_trend_weeks() -> u32 {
    12
}

fn default_history_limit() -> usize {
    10
}

fn default_page_limit() -> usize {
    25
}

#[derive(Serialize)]
struct WeightItem {
    timestamp: String,
    weight: f64,
}

impl From<WeightHistoryRow> for WeightItem {
    fn from(row: WeightHistoryRow) -> Self {
        Self {
            timestamp: row.timestamp,
            weight: row.weight,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PetSummary {
    pet_id: String,
    name: String,
    current_weight: f64,
    updated_at: String,
    recent_weights: Vec<WeightItem>,
    recent_visits: Vec<DailyVisitCount>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PetIdentity {
    pet_id: String,
    name: String,
    current_weight: f64,
    updated_at: String,
}

#[derive(Serialize)]
#[serde(tag = "resource", rename_all = "lowercase")]
enum PetsReadOutput {
    List {
        pets: Vec<PetSummary>,
    },
    #[serde(rename_all = "camelCase")]
    History {
        pet: PetIdentity,
        items: Vec<WeightItem>,
        next_cursor: Option<usize>,
        total: usize,
    },
    Trend {
        #[serde(flatten)]
        health: omni_api::pets::PetHealthResponse,
    },
}

/// Handler dependencies.
#[derive(Clone)]
struct PetsDeps {
    pets: PetStore,
    ledger: HealthLedger,
    clock: SharedClock,
    tz: TimeZone,
}

/// `values.slice(-n)`: the last `n`, or everything when `n` is 0 (`-0` is 0).
fn last_n<T>(mut values: Vec<T>, n: usize) -> Vec<T> {
    if n == 0 || n >= values.len() {
        return values;
    }
    values.split_off(values.len() - n)
}

async fn pets_read(deps: PetsDeps, input: PetsReadInput) -> Result<PetsReadOutput, ToolError> {
    let store_err = |e: omni_store::StoreError| ToolError::execute_from(&e);
    let pets = deps.pets;
    match input {
        PetsReadInput::List { history_limit } => {
            let mut out = Vec::new();
            for entry in pets.all_pets_with_history().await.map_err(store_err)? {
                let visits = pets
                    .daily_visit_counts(&entry.pet.pet_id)
                    .await
                    .map_err(store_err)?;
                out.push(PetSummary {
                    pet_id: entry.pet.pet_id,
                    name: entry.pet.name,
                    current_weight: entry.pet.current_weight,
                    updated_at: entry.pet.updated_at,
                    recent_weights: last_n(entry.weight_history, history_limit)
                        .into_iter()
                        .map(WeightItem::from)
                        .collect(),
                    recent_visits: last_n(visits, history_limit),
                });
            }
            Ok(PetsReadOutput::List { pets: out })
        }
        PetsReadInput::History {
            pet_id,
            cursor,
            limit,
        } => {
            let pet = pets
                .get_pet(&pet_id)
                .await
                .map_err(store_err)?
                .ok_or_else(|| ToolError::execute("Pet not found"))?;
            let history = pets.weight_history(&pet_id).await.map_err(store_err)?;
            let page = paginate(
                history.into_iter().map(WeightItem::from).collect(),
                cursor,
                limit,
            );
            Ok(PetsReadOutput::History {
                pet: PetIdentity {
                    pet_id: pet.pet_id,
                    name: pet.name,
                    current_weight: pet.current_weight,
                    updated_at: pet.updated_at,
                },
                items: page.items,
                next_cursor: page.next_cursor,
                total: page.total,
            })
        }
        PetsReadInput::Trend { pet_id, weeks } => {
            let mut health =
                health_response(&pets, &deps.ledger, deps.clock.now_ms(), &deps.tz, weeks)
                    .await
                    .map_err(store_err)?;
            if let Some(pet_id) = pet_id {
                if !health.pets.iter().any(|p| p.pet_id == pet_id) {
                    return Err(ToolError::execute("Pet not found"));
                }
                health.pets.retain(|p| p.pet_id == pet_id);
                health
                    .alerts
                    .retain(|a| a.pet_id.as_ref().is_none_or(|id| *id == pet_id));
            }
            Ok(PetsReadOutput::Trend { health })
        }
    }
}

#[derive(Deserialize)]
struct CostsReadInput {
    #[serde(default = "default_days")]
    days: f64,
}

fn default_days() -> f64 {
    30.0
}

/// Builds `pets_read` and `costs_read`.
pub fn tools(
    pets: PetStore,
    ledger: HealthLedger,
    store: Store,
    clock: SharedClock,
    tz: TimeZone,
) -> Result<Vec<McpTool>, ToolMetaError> {
    let deps = PetsDeps {
        pets,
        ledger,
        clock: clock.clone(),
        tz: tz.clone(),
    };
    let pets_tool = typed_tool(
        &defs::PETS_READ,
        move |input: PetsReadInput, _cx: ToolContext| {
            let deps = deps.clone();
            async move { pets_read(deps, input).await }
        },
    )?;
    let costs_tool = typed_tool(
        &defs::COSTS_READ,
        move |input: CostsReadInput, _cx: ToolContext| {
            let store = store.clone();
            let clock = clock.clone();
            let tz = tz.clone();
            async move {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let days = input.days as u32;
                let count = store
                    .read(|docs| docs.count::<CostEventData>())
                    .await
                    .map_err(|e| ToolError::execute_from(&e))?;
                if count > MAX_MCP_COST_EVENTS {
                    return Err(ToolError::execute(format!(
                        "Cost telemetry exceeds the MCP scan limit ({count} events; maximum {MAX_MCP_COST_EVENTS})"
                    )));
                }
                let events = store
                    .read(|docs| docs.get_all::<CostEventData>())
                    .await
                    .map_err(|e| ToolError::execute_from(&e))?;
                let summary = summarize(&events, CostRange::Days(days), clock.now_ms(), &tz);
                Ok::<_, ToolError>(summary)
            }
        },
    )?;
    Ok(vec![pets_tool, costs_tool])
}
