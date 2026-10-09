//! `briefing-history` and `briefing-delivery` (`src/briefing-agent/persistence.ts`).

use omni_store::cbor::{Extra, JsValue};
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{DocOps as _, Store, StoreError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::format::format_month_day_time;

/// Notifications kept per briefing.
pub const MAX_NOTIFICATIONS: usize = 50;

/// `costCents?: number | null`: never computed (absent), unpriced (`null`) or a value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CostCents {
    #[default]
    Absent,
    Unpriced,
    Cents(f64),
}

impl CostCents {
    pub fn is_absent(&self) -> bool {
        matches!(self, CostCents::Absent)
    }

    /// The API view: absent and unpriced both read as `null`.
    pub fn as_option(self) -> Option<f64> {
        match self {
            CostCents::Cents(cents) => Some(cents),
            CostCents::Absent | CostCents::Unpriced => None,
        }
    }
}

impl Serialize for CostCents {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CostCents::Cents(cents) => serializer.serialize_f64(*cents),
            CostCents::Absent | CostCents::Unpriced => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for CostCents {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match JsValue::deserialize(deserializer)? {
            JsValue::Undefined => Ok(CostCents::Absent),
            JsValue::Null => Ok(CostCents::Unpriced),
            other => other
                .as_f64()
                .map(CostCents::Cents)
                .ok_or_else(|| serde::de::Error::custom("costCents must be a number or null")),
        }
    }
}

/// `BriefingNotification`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingNotificationData {
    pub title: String,
    pub message: String,
    pub url: String,
    pub timestamp: i64,
    /// Task-run id this notification was produced by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "CostCents::is_absent")]
    pub cost_cents: CostCents,
    #[serde(flatten)]
    pub extra: Extra,
}

impl BriefingNotificationData {
    pub fn new(title: String, message: String, url: String, timestamp: i64) -> Self {
        Self {
            title,
            message,
            url,
            timestamp,
            run_id: None,
            cost_cents: CostCents::Absent,
            extra: Extra::new(),
        }
    }

    pub fn view(&self) -> omni_api::briefings::BriefingNotification {
        omni_api::briefings::BriefingNotification {
            title: self.title.clone(),
            message: self.message.clone(),
            url: self.url.clone(),
            timestamp: self.timestamp,
            run_id: self.run_id.clone(),
            cost_cents: self.cost_cents.as_option(),
        }
    }
}

/// `briefing-history`, keyed by `briefingName`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingHistoryData {
    pub briefing_name: String,
    pub notifications: Vec<BriefingNotificationData>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl BriefingHistoryData {
    pub fn empty(briefing_name: &str) -> Self {
        Self {
            briefing_name: briefing_name.to_owned(),
            notifications: Vec::new(),
            extra: Extra::new(),
        }
    }
}

impl Entity for BriefingHistoryData {
    const NAME: &'static str = "briefing-history";
    type Key = String;
    fn key(&self) -> String {
        self.briefing_name.clone()
    }
}

/// `BriefingDeliveryData["status"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    Sending,
    Delivered,
}

/// `briefing-delivery`, keyed by `briefingName, deliveryId`: one reservation per
/// `send_notification` tool call, kept permanently once delivered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingDeliveryData {
    pub briefing_name: String,
    pub delivery_id: String,
    pub status: DeliveryStatus,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for BriefingDeliveryData {
    const NAME: &'static str = "briefing-delivery";
    type Key = (String, String);
    fn key(&self) -> (String, String) {
        (self.briefing_name.clone(), self.delivery_id.clone())
    }
}

fn delivery(
    briefing_name: &str,
    delivery_id: &str,
    status: DeliveryStatus,
    now: i64,
) -> BriefingDeliveryData {
    BriefingDeliveryData {
        briefing_name: briefing_name.to_owned(),
        delivery_id: delivery_id.to_owned(),
        status,
        updated_at: now,
        extra: Extra::new(),
    }
}

/// Reserves one model tool call before Pushover so AI retries cannot duplicate
/// it; `false` when the id is already reserved or delivered.
pub async fn reserve_delivery(
    store: &Store,
    briefing_name: &str,
    delivery_id: &str,
) -> Result<bool, StoreError> {
    let (name, id) = (briefing_name.to_owned(), delivery_id.to_owned());
    store
        .write(move |tx| {
            let key = (name.clone(), id.clone());
            if tx.has::<BriefingDeliveryData>(&key)? {
                return Ok(false);
            }
            let now = tx.now_ms();
            tx.upsert(
                &delivery(&name, &id, DeliveryStatus::Sending, now),
                UpsertOpts::default(),
            )?;
            Ok(true)
        })
        .await
}

/// Keeps a successful reservation permanently as the idempotency record.
pub async fn complete_delivery(
    store: &Store,
    briefing_name: &str,
    delivery_id: &str,
) -> Result<(), StoreError> {
    let (name, id) = (briefing_name.to_owned(), delivery_id.to_owned());
    store
        .write(move |tx| {
            let now = tx.now_ms();
            tx.upsert(
                &delivery(&name, &id, DeliveryStatus::Delivered, now),
                UpsertOpts::default(),
            )
        })
        .await
}

/// A confirmed provider failure is safe to retry.
pub async fn release_delivery(
    store: &Store,
    briefing_name: &str,
    delivery_id: &str,
) -> Result<(), StoreError> {
    let key = (briefing_name.to_owned(), delivery_id.to_owned());
    store
        .write(move |tx| tx.delete::<BriefingDeliveryData>(&key).map(drop))
        .await
}

/// The stored history, or an empty one.
pub async fn get_history(
    store: &Store,
    briefing_name: &str,
) -> Result<BriefingHistoryData, StoreError> {
    let name = briefing_name.to_owned();
    store
        .read(move |docs| {
            Ok(docs
                .get::<BriefingHistoryData>(&name)?
                .unwrap_or_else(|| BriefingHistoryData::empty(&name)))
        })
        .await
}

/// Every stored history.
pub async fn get_all_histories(store: &Store) -> Result<Vec<BriefingHistoryData>, StoreError> {
    store
        .read(|docs| docs.get_all::<BriefingHistoryData>())
        .await
}

/// Appends one notification atomically, keeping the newest [`MAX_NOTIFICATIONS`].
pub async fn add_notification(
    store: &Store,
    briefing_name: &str,
    notification: BriefingNotificationData,
) -> Result<(), StoreError> {
    let name = briefing_name.to_owned();
    store
        .write(move |tx| {
            let mut history = tx
                .get::<BriefingHistoryData>(&name)?
                .unwrap_or_else(|| BriefingHistoryData::empty(&name));
            history.briefing_name.clone_from(&name);
            history.notifications.push(notification);
            let excess = history
                .notifications
                .len()
                .saturating_sub(MAX_NOTIFICATIONS);
            history.notifications.drain(..excess);
            tx.upsert(&history, UpsertOpts::default())
        })
        .await
}

/// Backfills a run's total LLM cost across the notifications it produced
/// (evenly); without a run id, onto the last notification. No-op when the run
/// produced none.
pub async fn distribute_run_cost(
    store: &Store,
    briefing_name: &str,
    run_id: Option<&str>,
    total_cost_cents: Option<f64>,
) -> Result<(), StoreError> {
    let name = briefing_name.to_owned();
    let run_id = run_id.map(str::to_owned);
    store
        .write(move |tx| {
            let Some(mut history) = tx.get::<BriefingHistoryData>(&name)? else {
                return Ok(());
            };
            let own: Vec<usize> = match &run_id {
                Some(run_id) => history
                    .notifications
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.run_id.as_deref() == Some(run_id.as_str()))
                    .map(|(index, _)| index)
                    .collect(),
                None => history
                    .notifications
                    .len()
                    .checked_sub(1)
                    .into_iter()
                    .collect(),
            };
            if own.is_empty() {
                return Ok(());
            }
            #[allow(clippy::cast_precision_loss)]
            let per = match total_cost_cents {
                Some(total) => CostCents::Cents(total / own.len() as f64),
                None => CostCents::Unpriced,
            };
            for index in own {
                if let Some(notification) = history.notifications.get_mut(index) {
                    notification.cost_cents = per;
                }
            }
            tx.upsert(&history, UpsertOpts::default())
        })
        .await
}

/// `formatNotifications`: the newest `count` as `- title (url) [Feb 6, 2:30 PM]`.
pub fn format_notifications(
    notifications: &[BriefingNotificationData],
    count: i64,
    tz: &jiff::tz::TimeZone,
) -> String {
    if count <= 0 || notifications.is_empty() {
        return "- No previous notifications".to_owned();
    }
    let take = usize::try_from(count)
        .unwrap_or(usize::MAX)
        .min(notifications.len());
    notifications[notifications.len() - take..]
        .iter()
        .map(|n| {
            format!(
                "- {} ({}) [{}]",
                n.title,
                n.url,
                format_month_day_time(n.timestamp, tz)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `resolveHistoryPlaceholders`: replaces every `{{history:N}}` with the
/// briefing's newest `N` notifications.
pub async fn resolve_history_placeholders(
    store: &Store,
    prompt: &str,
    briefing_name: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<String, StoreError> {
    let matches = history_placeholders(prompt);
    if matches.is_empty() {
        return Ok(prompt.to_owned());
    }
    let history = get_history(store, briefing_name).await?;
    let mut resolved = prompt.to_owned();
    for (token, count) in matches {
        let replacement = format_notifications(&history.notifications, count, tz);
        resolved = crate::format::js_replace_first(&resolved, &token, &replacement);
    }
    Ok(resolved)
}

/// Every `{{history:N}}` occurrence with its parsed count, in order.
fn history_placeholders(prompt: &str) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let mut rest = prompt;
    while let Some(start) = rest.find("{{history:") {
        let after = &rest[start + "{{history:".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with("}}") {
            let token = &rest[start..start + "{{history:".len() + digits + 2];
            // `Number.parseInt` of an over-long run of digits is a huge count:
            // every notification is shown either way.
            let count = after[..digits].parse::<i64>().unwrap_or(i64::MAX);
            out.push((token.to_owned(), count));
            rest = &rest[start + token.len()..];
        } else {
            rest = &rest[start + 2..];
        }
    }
    out
}
