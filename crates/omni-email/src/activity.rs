//! Per-email pipeline outcomes (`src/email/activity.ts`): one `email-activity`
//! row per pipeline and email, overwritten on reprocess and pruned to the
//! newest [`KEEP_PER_PIPELINE`] rows per pipeline together with their logs.

use omni_core::email::FetchedEmail;
use omni_store::cbor::{self, Extra, JsValue};
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::activity_logs::EmailActivityLog;

/// Rows kept per pipeline; older rows (and their logs) are pruned on record.
pub const KEEP_PER_PIPELINE: usize = 1000;

pub use omni_api::email::{AdmitTier, EmailActivityOutcome, EmailPipelineName};

/// `deriveItemsOutcome`: outcome from per-item success flags (empty is `no_matches`).
/// A fully rejected submission is `failed`, never `processed`.
pub fn derive_items_outcome(items_ok: &[bool]) -> EmailActivityOutcome {
    if items_ok.is_empty() {
        return EmailActivityOutcome::NoMatches;
    }
    let succeeded = items_ok.iter().filter(|ok| **ok).count();
    if succeeded == items_ok.len() {
        EmailActivityOutcome::Processed
    } else if succeeded == 0 {
        EmailActivityOutcome::Failed
    } else {
        EmailActivityOutcome::Partial
    }
}

/// LLM cost attributed to an activity row (TS `number | null | undefined`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LlmCost {
    /// No attributable call ran (`undefined`; the field is omitted).
    #[default]
    None,
    /// A call ran on a model without a price (`null`).
    Unpriced,
    /// USD cents.
    Cents(f64),
}

impl LlmCost {
    pub fn is_none(&self) -> bool {
        matches!(self, LlmCost::None)
    }

    /// `costCents ?? null` as served by the API and MCP tools.
    pub fn as_nullable(&self) -> Option<f64> {
        match self {
            LlmCost::Cents(cents) => Some(*cents),
            LlmCost::None | LlmCost::Unpriced => None,
        }
    }

    /// From a priced/unpriced call result (`hasPrice ? cents : null`).
    pub fn from_call(cents: Option<f64>) -> Self {
        cents.map_or(LlmCost::Unpriced, LlmCost::Cents)
    }
}

/// `sumCostCents`: `None` parts are dropped; all `None` gives `None`; any
/// `Unpriced` part makes the total `Unpriced`.
pub fn sum_cost_cents(parts: &[LlmCost]) -> LlmCost {
    let known: Vec<&LlmCost> = parts.iter().filter(|p| !p.is_none()).collect();
    if known.is_empty() {
        return LlmCost::None;
    }
    if known.iter().any(|p| matches!(p, LlmCost::Unpriced)) {
        return LlmCost::Unpriced;
    }
    LlmCost::Cents(known.iter().map(|p| p.as_nullable().unwrap_or(0.0)).sum())
}

fn serialize_cost<S: Serializer>(cost: &LlmCost, serializer: S) -> Result<S::Ok, S::Error> {
    match cost {
        LlmCost::Cents(cents) => serializer.serialize_f64(*cents),
        LlmCost::None | LlmCost::Unpriced => serializer.serialize_none(),
    }
}

fn deserialize_cost<'de, D: Deserializer<'de>>(deserializer: D) -> Result<LlmCost, D::Error> {
    match JsValue::deserialize(deserializer)? {
        JsValue::Undefined => Ok(LlmCost::None),
        JsValue::Null => Ok(LlmCost::Unpriced),
        other => cbor::from_value::<f64>(other)
            .map(LlmCost::Cents)
            .map_err(serde::de::Error::custom),
    }
}

/// `EmailActivityData` (entity `email-activity`, key `activityId`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailActivityData {
    /// `<pipeline>#<emailId>`.
    pub activity_id: String,
    pub pipeline: EmailPipelineName,
    pub email_id: String,
    pub subject: String,
    pub from: String,
    pub received_at: i64,
    pub processed_at: i64,
    pub outcome: EmailActivityOutcome,
    /// Filter reason, error message, or other context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admit_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admit_tier: Option<AdmitTier>,
    #[serde(
        default,
        skip_serializing_if = "LlmCost::is_none",
        serialize_with = "serialize_cost",
        deserialize_with = "deserialize_cost"
    )]
    pub cost_cents: LlmCost,
    /// Short per-item results, e.g. `1Z999AA1 (ups): submitted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailActivityData {
    const NAME: &'static str = "email-activity";
    type Key = String;
    fn key(&self) -> String {
        self.activity_id.clone()
    }
}

/// `<pipeline>#<emailId>`.
pub fn activity_id(pipeline: EmailPipelineName, email_id: &str) -> String {
    format!("{pipeline}#{email_id}")
}

/// The email fields an activity row keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityEmail {
    pub id: String,
    pub subject: String,
    pub from: String,
    /// ISO-8601 (`FetchedEmail.receivedAt`).
    pub received_at: String,
}

impl From<&FetchedEmail> for ActivityEmail {
    fn from(email: &FetchedEmail) -> Self {
        Self {
            id: email.id.clone(),
            subject: email.subject.clone(),
            from: email.from.clone(),
            received_at: email.received_at.clone(),
        }
    }
}

/// One `recordEmailActivity` call.
#[derive(Clone, Debug, PartialEq)]
pub struct NewActivity {
    pub pipeline: EmailPipelineName,
    pub email: ActivityEmail,
    pub outcome: EmailActivityOutcome,
    pub detail: Option<String>,
    pub admit_reason: Option<String>,
    pub admit_tier: Option<AdmitTier>,
    pub cost_cents: LlmCost,
    pub items: Option<Vec<String>>,
}

impl NewActivity {
    /// An activity with only the required fields set.
    pub fn new(
        pipeline: EmailPipelineName,
        email: impl Into<ActivityEmail>,
        outcome: EmailActivityOutcome,
    ) -> Self {
        Self {
            pipeline,
            email: email.into(),
            outcome,
            detail: None,
            admit_reason: None,
            admit_tier: None,
            cost_cents: LlmCost::None,
            items: None,
        }
    }
}

/// `Date.parse` for the ISO strings transports produce (always with an offset,
/// so the zone for offset-less input does not matter); `None` when unparseable.
pub fn parse_js_date_ms(value: &str) -> Option<i64> {
    omni_core::js::date_parse(value, &jiff::tz::TimeZone::UTC)
}

/// `selectActivityToPrune`: rows beyond the newest `keep` for one pipeline.
pub fn select_activity_to_prune(
    all: &[EmailActivityData],
    pipeline: EmailPipelineName,
    keep: usize,
) -> Vec<EmailActivityData> {
    let mut rows: Vec<&EmailActivityData> = all.iter().filter(|a| a.pipeline == pipeline).collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.processed_at));
    rows.into_iter().skip(keep).cloned().collect()
}

/// `recordEmailActivity`: upserts the row and prunes the pipeline's history
/// (activity rows and their logs) in one transaction.
pub async fn record(store: &Store, entry: NewActivity) -> Result<EmailActivityData, StoreError> {
    let now = store.clock().now_ms();
    let row = EmailActivityData {
        activity_id: activity_id(entry.pipeline, &entry.email.id),
        pipeline: entry.pipeline,
        email_id: entry.email.id,
        subject: entry.email.subject,
        from: entry.email.from,
        received_at: parse_js_date_ms(&entry.email.received_at).unwrap_or(now),
        processed_at: now,
        outcome: entry.outcome,
        detail: entry.detail,
        admit_reason: entry.admit_reason,
        admit_tier: entry.admit_tier,
        cost_cents: entry.cost_cents,
        items: entry.items,
        extra: Extra::new(),
    };
    let written = row.clone();
    store
        .write(move |tx| {
            tx.upsert(&row, UpsertOpts::default())?;
            let all = tx.get_all::<EmailActivityData>()?;
            for stale in select_activity_to_prune(&all, row.pipeline, KEEP_PER_PIPELINE) {
                tx.delete::<EmailActivityData>(&stale.activity_id)?;
                tx.delete::<EmailActivityLog>(&stale.activity_id)?;
            }
            Ok::<_, StoreError>(())
        })
        .await?;
    Ok(written)
}

/// `getEmailActivity`.
pub async fn get(
    store: &Store,
    activity_id: &str,
) -> Result<Option<EmailActivityData>, StoreError> {
    let key = activity_id.to_owned();
    store
        .read(move |docs| docs.get::<EmailActivityData>(&key))
        .await
}

/// `getRecentEmailActivity`: newest first, optionally one pipeline, at most `limit`.
pub async fn recent(
    store: &Store,
    pipeline: Option<EmailPipelineName>,
    limit: usize,
) -> Result<Vec<EmailActivityData>, StoreError> {
    let mut rows: Vec<EmailActivityData> = store
        .read(|docs| docs.get_all::<EmailActivityData>())
        .await?
        .into_iter()
        .filter(|a| pipeline.is_none_or(|p| a.pipeline == p))
        .collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.processed_at));
    rows.truncate(limit);
    Ok(rows)
}

#[cfg(test)]
mod activity_spec {
    use super::*;

    fn make_activity(
        activity_id: &str,
        pipeline: EmailPipelineName,
        processed_at: i64,
    ) -> EmailActivityData {
        EmailActivityData {
            activity_id: activity_id.to_owned(),
            pipeline,
            email_id: "email-1".to_owned(),
            subject: "Your order shipped".to_owned(),
            from: "shop@example.com".to_owned(),
            received_at: 1_000,
            processed_at,
            outcome: EmailActivityOutcome::Processed,
            detail: None,
            admit_reason: None,
            admit_tier: None,
            cost_cents: LlmCost::None,
            items: None,
            extra: Extra::new(),
        }
    }

    const PARCEL: EmailPipelineName = EmailPipelineName::ParcelTracker;
    const CALENDAR: EmailPipelineName = EmailPipelineName::CalendarEvents;

    #[test]
    fn returns_nothing_when_under_the_cap() {
        let all = [
            make_activity("ParcelTracker#a", PARCEL, 1),
            make_activity("ParcelTracker#b", PARCEL, 2),
        ];
        assert!(select_activity_to_prune(&all, PARCEL, 5).is_empty());
    }

    #[test]
    fn returns_the_oldest_rows_beyond_the_cap() {
        let all = [
            make_activity("ParcelTracker#a", PARCEL, 1),
            make_activity("ParcelTracker#b", PARCEL, 3),
            make_activity("ParcelTracker#c", PARCEL, 2),
        ];
        let pruned = select_activity_to_prune(&all, PARCEL, 2);
        let ids: Vec<&str> = pruned.iter().map(|a| a.activity_id.as_str()).collect();
        assert_eq!(ids, ["ParcelTracker#a"]);
    }

    #[test]
    fn only_prunes_rows_for_the_given_pipeline() {
        let all = [
            make_activity("ParcelTracker#a", PARCEL, 1),
            make_activity("CalendarEvents#b", CALENDAR, 2),
        ];
        assert!(select_activity_to_prune(&all, PARCEL, 1).is_empty());
        assert_eq!(select_activity_to_prune(&all, CALENDAR, 0).len(), 1);
    }

    #[test]
    fn sums_costs_like_ts() {
        assert_eq!(
            sum_cost_cents(&[LlmCost::None, LlmCost::None]),
            LlmCost::None
        );
        assert_eq!(sum_cost_cents(&[]), LlmCost::None);
        assert_eq!(
            sum_cost_cents(&[LlmCost::Cents(1.5), LlmCost::None, LlmCost::Cents(2.0)]),
            LlmCost::Cents(3.5)
        );
        assert_eq!(
            sum_cost_cents(&[LlmCost::Cents(1.5), LlmCost::Unpriced]),
            LlmCost::Unpriced
        );
    }

    #[test]
    fn parses_js_dates() {
        assert_eq!(parse_js_date_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_js_date_ms("2026-09-01T00:00:00Z"),
            Some(1_788_220_800_000)
        );
        assert_eq!(parse_js_date_ms("2026-09-01"), Some(1_788_220_800_000));
        assert_eq!(parse_js_date_ms("garbage"), None);
    }
}
