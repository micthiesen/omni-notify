//! The cached Parcel deliveries and the durable read budget.
//!
//! Parcel allows very few reads (about 20 per hour, failed requests included),
//! so every read is reserved here before the request: the reservation checks
//! the schedule, the 429 back-off and the hourly budget in one write
//! transaction, then records the attempt. Nothing else calls Parcel.

use omni_api::parcels::ParcelDeliveryStatus;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

use super::api::RawDelivery;
use crate::persistence::{SubmissionStatus, SubmittedDelivery};

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
/// Between reads while any delivery is active (or nothing is known yet).
pub const ACTIVE_INTERVAL_MS: i64 = 30 * MINUTE_MS;
/// Between reads while the last read showed nothing active.
pub const IDLE_INTERVAL_MS: i64 = 3 * HOUR_MS;
/// Ticks land a little early or late; a tick this close to due counts as due.
pub const DUE_SLACK_MS: i64 = 2 * MINUTE_MS;
/// Reads (attempts, failed ones included) allowed in any rolling hour.
pub const HOURLY_BUDGET: usize = 4;
pub const BUDGET_WINDOW_MS: i64 = HOUR_MS;
/// After a 429, the minimum pause.
pub const RATE_LIMIT_BACKOFF_MS: i64 = HOUR_MS;
/// A `Retry-After` longer than this is capped.
pub const MAX_BACKOFF_MS: i64 = 24 * HOUR_MS;

/// Stored bounds.
pub const MAX_DELIVERIES: usize = 100;
pub const MAX_EVENTS: usize = 30;
const MAX_TEXT_CHARS: usize = 500;
const MAX_ERROR_CHARS: usize = 300;

/// The single row key of both entities.
pub const KEY: &str = "parcel";

/// One carrier event, as cached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedEvent {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional: Option<String>,
}

/// One delivery, as cached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedDelivery {
    pub tracking_number: String,
    pub carrier_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier_name: Option<String>,
    pub description: String,
    pub status_code: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_information: Option<String>,
    /// Newest first, at most [`MAX_EVENTS`].
    pub events: Vec<CachedEvent>,
    pub event_count: u32,
}

impl CachedDelivery {
    pub fn status(&self) -> ParcelDeliveryStatus {
        ParcelDeliveryStatus::from_code(self.status_code)
    }
}

/// The last successful read (entity `parcel-deliveries-snapshot`, key
/// [`KEY`]); replaced whole by each successful read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveriesSnapshot {
    pub key: String,
    pub fetched_at: i64,
    pub deliveries: Vec<CachedDelivery>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for DeliveriesSnapshot {
    const NAME: &'static str = "parcel-deliveries-snapshot";
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
}

impl DeliveriesSnapshot {
    pub fn active_count(&self) -> usize {
        self.deliveries
            .iter()
            .filter(|d| d.status().is_active())
            .count()
    }
}

/// Read attempts and their outcome (entity `parcel-deliveries-read-state`,
/// key [`KEY`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadState {
    pub key: String,
    /// Attempt times inside the budget window, oldest first.
    #[serde(default)]
    pub attempts: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_until: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<u16>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for ReadState {
    const NAME: &'static str = "parcel-deliveries-read-state";
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
}

/// Why a tick does not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Parcel answered 429.
    BackingOff,
    /// The interval since the last attempt has not passed.
    NotDue,
    /// [`HOURLY_BUDGET`] attempts already happened in the last hour.
    BudgetExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadDecision {
    Read,
    Skip { reason: SkipReason, until: i64 },
}

/// Whether the schedule wants the 30-minute cadence: no successful read yet,
/// an active delivery in the last read, or a tracking number Omni submitted
/// after it.
pub fn wants_active_cadence(
    snapshot: Option<&DeliveriesSnapshot>,
    submissions: &[SubmittedDelivery],
) -> bool {
    let Some(snapshot) = snapshot else {
        return true;
    };
    snapshot.active_count() > 0
        || submissions.iter().any(|row| {
            row.status != Some(SubmissionStatus::Rejected) && row.submitted_at > snapshot.fetched_at
        })
}

/// When the next read may happen, ignoring the budget.
pub fn next_due(state: &ReadState, active: bool) -> Option<i64> {
    let interval = if active {
        ACTIVE_INTERVAL_MS
    } else {
        IDLE_INTERVAL_MS
    };
    let due = state.last_attempt_at.map(|at| at + interval - DUE_SLACK_MS);
    match (due, state.backoff_until) {
        (Some(due), Some(backoff)) => Some(due.max(backoff)),
        (due, backoff) => due.or(backoff),
    }
}

/// The schedule, back-off and budget decision for a tick at `now`.
pub fn decide(state: &ReadState, active: bool, now: i64) -> ReadDecision {
    if let Some(until) = state.backoff_until
        && now < until
    {
        return ReadDecision::Skip {
            reason: SkipReason::BackingOff,
            until,
        };
    }
    if let Some(due) = next_due(state, active)
        && now < due
    {
        return ReadDecision::Skip {
            reason: SkipReason::NotDue,
            until: due,
        };
    }
    let recent: Vec<i64> = state
        .attempts
        .iter()
        .copied()
        .filter(|at| *at > now - BUDGET_WINDOW_MS && *at <= now)
        .collect();
    if recent.len() >= HOURLY_BUDGET {
        let oldest = recent.iter().copied().min().unwrap_or(now);
        return ReadDecision::Skip {
            reason: SkipReason::BudgetExhausted,
            until: oldest + BUDGET_WINDOW_MS,
        };
    }
    ReadDecision::Read
}

fn bounded(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn bounded_opt(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| bounded(t, MAX_TEXT_CHARS))
}

/// Parcel's deliveries, bounded for storage, with carrier names resolved by
/// `carrier_name`.
pub fn to_cached(
    raw: Vec<RawDelivery>,
    carrier_name: impl Fn(&str) -> Option<String>,
) -> Vec<CachedDelivery> {
    raw.into_iter()
        .take(MAX_DELIVERIES)
        .map(|delivery| {
            let carrier_code = delivery.carrier_code.unwrap_or_default();
            let event_count = u32::try_from(delivery.events.len()).unwrap_or(u32::MAX);
            CachedDelivery {
                tracking_number: bounded(delivery.tracking_number.trim(), 200),
                carrier_name: carrier_name(&carrier_code),
                carrier_code,
                description: bounded(
                    delivery.description.as_deref().unwrap_or("").trim(),
                    MAX_TEXT_CHARS,
                ),
                status_code: delivery.status_code,
                expected: bounded_opt(delivery.date_expected.as_deref()),
                expected_end: bounded_opt(delivery.date_expected_end.as_deref()),
                extra_information: bounded_opt(delivery.extra_information.as_deref()),
                events: delivery
                    .events
                    .into_iter()
                    .take(MAX_EVENTS)
                    .map(|event| CachedEvent {
                        description: bounded(
                            event.event.as_deref().unwrap_or("").trim(),
                            MAX_TEXT_CHARS,
                        ),
                        date: bounded_opt(event.date.as_deref()),
                        location: bounded_opt(event.location.as_deref()),
                        additional: bounded_opt(event.additional.as_deref()),
                    })
                    .collect(),
                event_count,
            }
        })
        .collect()
}

/// What [`reserve`] decided, with the state it saw.
#[derive(Clone, Debug, PartialEq)]
pub struct Reservation {
    pub decision: ReadDecision,
    pub active: bool,
}

/// Decides and, for a read, durably records the attempt before Parcel is
/// called, so an interrupted or failed request still counts against the budget.
pub async fn reserve(store: &Store, now: i64) -> Result<Reservation, StoreError> {
    store
        .write(move |tx| {
            let snapshot = tx.get::<DeliveriesSnapshot>(&KEY.to_owned())?;
            let submissions = match &snapshot {
                Some(s) if s.active_count() == 0 => tx.get_all::<SubmittedDelivery>()?,
                _ => Vec::new(),
            };
            let active = wants_active_cadence(snapshot.as_ref(), &submissions);
            let mut state = tx.get::<ReadState>(&KEY.to_owned())?.unwrap_or(ReadState {
                key: KEY.to_owned(),
                ..ReadState::default()
            });
            let decision = decide(&state, active, now);
            if decision == ReadDecision::Read {
                state
                    .attempts
                    .retain(|at| *at > now - BUDGET_WINDOW_MS && *at <= now);
                state.attempts.push(now);
                state.last_attempt_at = Some(now);
                tx.upsert(&state, UpsertOpts::default())?;
            }
            Ok::<_, StoreError>(Reservation { decision, active })
        })
        .await
}

/// Replaces the snapshot and clears the failure.
pub async fn record_success(
    store: &Store,
    now: i64,
    deliveries: Vec<CachedDelivery>,
) -> Result<DeliveriesSnapshot, StoreError> {
    store
        .write(move |tx| {
            let key = KEY.to_owned();
            let extra = tx
                .get::<DeliveriesSnapshot>(&key)?
                .map(|s| s.extra)
                .unwrap_or_default();
            let snapshot = DeliveriesSnapshot {
                key: key.clone(),
                fetched_at: now,
                deliveries,
                extra,
            };
            tx.upsert(&snapshot, UpsertOpts::default())?;
            let mut state = tx.get::<ReadState>(&key)?.unwrap_or(ReadState {
                key,
                ..ReadState::default()
            });
            state.last_success_at = Some(now);
            state.last_error = None;
            state.last_status = Some(200);
            state.backoff_until = None;
            tx.upsert(&state, UpsertOpts::default())?;
            Ok::<_, StoreError>(snapshot)
        })
        .await
}

/// Records a failed read; `backoff_until` is set for a 429.
pub async fn record_failure(
    store: &Store,
    message: &str,
    status: Option<u16>,
    backoff_until: Option<i64>,
) -> Result<(), StoreError> {
    let message = bounded(message, MAX_ERROR_CHARS);
    store
        .write(move |tx| {
            let key = KEY.to_owned();
            let mut state = tx.get::<ReadState>(&key)?.unwrap_or(ReadState {
                key,
                ..ReadState::default()
            });
            state.last_error = Some(message);
            state.last_status = status;
            if backoff_until.is_some() {
                state.backoff_until = backoff_until;
            }
            tx.upsert(&state, UpsertOpts::default())?;
            Ok::<_, StoreError>(())
        })
        .await
}

/// The cached snapshot and read state, for the route and MCP tools.
pub async fn load(
    store: &Store,
) -> Result<
    (
        Option<DeliveriesSnapshot>,
        Option<ReadState>,
        Vec<SubmittedDelivery>,
    ),
    StoreError,
> {
    store
        .read(|docs| {
            let key = KEY.to_owned();
            Ok::<_, StoreError>((
                docs.get::<DeliveriesSnapshot>(&key)?,
                docs.get::<ReadState>(&key)?,
                docs.get_all::<SubmittedDelivery>()?,
            ))
        })
        .await
}

#[cfg(test)]
mod state_spec {
    use super::*;

    const NOW: i64 = 1_790_000_000_000;

    fn state(attempts: &[i64], backoff: Option<i64>) -> ReadState {
        ReadState {
            key: KEY.to_owned(),
            attempts: attempts.to_vec(),
            last_attempt_at: attempts.iter().copied().max(),
            backoff_until: backoff,
            ..ReadState::default()
        }
    }

    fn delivery(status_code: i64) -> CachedDelivery {
        CachedDelivery {
            tracking_number: "T1".to_owned(),
            carrier_code: "ups".to_owned(),
            carrier_name: None,
            description: String::new(),
            status_code,
            expected: None,
            expected_end: None,
            extra_information: None,
            events: Vec::new(),
            event_count: 0,
        }
    }

    fn snapshot(fetched_at: i64, codes: &[i64]) -> DeliveriesSnapshot {
        DeliveriesSnapshot {
            key: KEY.to_owned(),
            fetched_at,
            deliveries: codes.iter().map(|c| delivery(*c)).collect(),
            extra: Extra::default(),
        }
    }

    fn submission(at: i64, status: Option<SubmissionStatus>) -> SubmittedDelivery {
        SubmittedDelivery {
            tracking_number: "T2".to_owned(),
            carrier_code: "ups".to_owned(),
            description: String::new(),
            submitted_at: at,
            email_id: "m".to_owned(),
            status,
            attempts: None,
            extra: Extra::default(),
        }
    }

    #[test]
    fn reads_immediately_when_nothing_was_ever_attempted() {
        assert_eq!(decide(&state(&[], None), true, NOW), ReadDecision::Read);
        assert_eq!(decide(&state(&[], None), false, NOW), ReadDecision::Read);
    }

    #[test]
    fn reads_every_30_minutes_while_active() {
        let last = NOW - 20 * MINUTE_MS;
        assert_eq!(
            decide(&state(&[last], None), true, NOW),
            ReadDecision::Skip {
                reason: SkipReason::NotDue,
                until: last + ACTIVE_INTERVAL_MS - DUE_SLACK_MS,
            }
        );
        let last = NOW - 29 * MINUTE_MS;
        assert_eq!(decide(&state(&[last], None), true, NOW), ReadDecision::Read);
    }

    #[test]
    fn reads_every_3_hours_while_idle() {
        let last = NOW - 2 * HOUR_MS;
        assert!(matches!(
            decide(&state(&[last], None), false, NOW),
            ReadDecision::Skip {
                reason: SkipReason::NotDue,
                ..
            }
        ));
        let last = NOW - 3 * HOUR_MS;
        assert_eq!(
            decide(&state(&[last], None), false, NOW),
            ReadDecision::Read
        );
    }

    #[test]
    fn backs_off_after_a_429_until_the_deadline() {
        let s = state(&[NOW - 2 * HOUR_MS], Some(NOW + 10 * MINUTE_MS));
        assert_eq!(
            decide(&s, true, NOW),
            ReadDecision::Skip {
                reason: SkipReason::BackingOff,
                until: NOW + 10 * MINUTE_MS,
            }
        );
        assert_eq!(decide(&s, true, NOW + 10 * MINUTE_MS), ReadDecision::Read);
    }

    #[test]
    fn refuses_a_fifth_attempt_within_an_hour() {
        // Only reachable when the schedule state is inconsistent (for
        // example an old `lastAttemptAt`); the budget still holds.
        let mut s = state(
            &[
                NOW - 50 * MINUTE_MS,
                NOW - 40 * MINUTE_MS,
                NOW - 35 * MINUTE_MS,
                NOW - 31 * MINUTE_MS,
            ],
            None,
        );
        s.last_attempt_at = Some(NOW - 2 * HOUR_MS);
        assert_eq!(
            decide(&s, true, NOW),
            ReadDecision::Skip {
                reason: SkipReason::BudgetExhausted,
                until: NOW + 10 * MINUTE_MS,
            }
        );
        s.attempts.remove(0);
        assert_eq!(decide(&s, true, NOW), ReadDecision::Read);
    }

    #[test]
    fn uses_the_active_cadence_without_a_snapshot_or_with_active_deliveries() {
        assert!(wants_active_cadence(None, &[]));
        assert!(wants_active_cadence(Some(&snapshot(NOW, &[0, 2])), &[]));
        assert!(!wants_active_cadence(Some(&snapshot(NOW, &[0, 0])), &[]));
    }

    #[test]
    fn a_newer_submission_restores_the_active_cadence() {
        let idle = snapshot(NOW, &[0]);
        assert!(wants_active_cadence(
            Some(&idle),
            &[submission(NOW + 1, Some(SubmissionStatus::Submitted))]
        ));
        assert!(wants_active_cadence(
            Some(&idle),
            &[submission(NOW + 1, None)]
        ));
        assert!(!wants_active_cadence(
            Some(&idle),
            &[submission(NOW + 1, Some(SubmissionStatus::Rejected))]
        ));
        assert!(!wants_active_cadence(
            Some(&idle),
            &[submission(NOW - 1, Some(SubmissionStatus::Submitted))]
        ));
    }

    #[test]
    fn bounds_events_and_text() {
        let raw = RawDelivery {
            tracking_number: " T1 ".to_owned(),
            carrier_code: Some("ups".to_owned()),
            description: Some("x".repeat(600)),
            status_code: 2,
            extra_information: Some("  ".to_owned()),
            date_expected: None,
            date_expected_end: None,
            events: (0..40)
                .map(|i| super::super::api::RawEvent {
                    event: Some(format!("e{i}")),
                    date: None,
                    location: Some(String::new()),
                    additional: None,
                })
                .collect(),
        };
        let cached = to_cached(vec![raw], |code| (code == "ups").then(|| "UPS".to_owned()));
        assert_eq!(cached[0].tracking_number, "T1");
        assert_eq!(cached[0].carrier_name.as_deref(), Some("UPS"));
        assert_eq!(cached[0].description.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(cached[0].extra_information, None);
        assert_eq!(cached[0].events.len(), MAX_EVENTS);
        assert_eq!(cached[0].event_count, 40);
        assert_eq!(cached[0].events[0].location, None);
    }
}
