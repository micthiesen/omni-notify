//! Durable retry queue for transiently failed email processing.
//! A retry re-fetches the email by id and reruns the
//! owning pipeline's handler; pipeline dedup gates make replay idempotent.
//! Rows parked by a systemic failure (`awaitingBuild`) are not due until a
//! boot under a different build releases them (see [`crate::systemic`]).

use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

/// Attempts before a row is dropped.
pub const MAX_RETRY_ATTEMPTS: i64 = 5;
/// 30 minutes, doubling per attempt.
const BASE_DELAY_MS: i64 = 30 * 60_000;

/// `EmailRetryData` (entity `email-retry`, key `retryKey`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailRetryData {
    /// `<pipeline>#<emailId>`.
    pub retry_key: String,
    pub pipeline: String,
    pub email_id: String,
    pub reason: String,
    /// Counts enqueue signals without consuming retry attempts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enqueue_count: Option<i64>,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub created_at: i64,
    /// Set while the row waits for a build other than this one (systemic failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_build: Option<String>,
    /// The systemic failure's signature key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailRetryData {
    const NAME: &'static str = "email-retry";
    type Key = String;
    fn key(&self) -> String {
        self.retry_key.clone()
    }
}

/// `<pipeline>#<emailId>`.
pub fn retry_key(pipeline: &str, email_id: &str) -> String {
    format!("{pipeline}#{email_id}")
}

/// 30 min x 2^(attempts-1); attempt 0 counts as the first.
pub fn retry_delay_ms(attempts: i64) -> i64 {
    let exponent = u32::try_from((attempts - 1).max(0))
        .unwrap_or(u32::MAX)
        .min(40);
    BASE_DELAY_MS.saturating_mul(1_i64 << exponent)
}

/// Coalesces repeated signals without consuming
/// attempts or moving an existing schedule. A transient signal unparks a row
/// parked by a systemic failure and schedules it normally.
pub fn plan_enqueue(
    existing: Option<&EmailRetryData>,
    pipeline: &str,
    email_id: &str,
    reason: &str,
    now: i64,
) -> EmailRetryData {
    let scheduled = existing.filter(|e| e.awaiting_build.is_none());
    EmailRetryData {
        retry_key: retry_key(pipeline, email_id),
        pipeline: pipeline.to_owned(),
        email_id: email_id.to_owned(),
        reason: reason.to_owned(),
        enqueue_count: Some(existing.and_then(|e| e.enqueue_count).unwrap_or(0) + 1),
        attempts: existing.map_or(0, |e| e.attempts),
        next_attempt_at: scheduled.map_or(now + retry_delay_ms(1), |e| e.next_attempt_at),
        created_at: existing.map_or(now, |e| e.created_at),
        awaiting_build: None,
        signature: None,
        extra: existing.map(|e| e.extra.clone()).unwrap_or_default(),
    }
}

/// Parks the row until a build other than `build` runs, keeping its attempts
/// and creation time; counts as an enqueue signal.
pub fn plan_enqueue_systemic(
    existing: Option<&EmailRetryData>,
    pipeline: &str,
    email_id: &str,
    reason: &str,
    signature: &str,
    build: &str,
    now: i64,
) -> EmailRetryData {
    EmailRetryData {
        awaiting_build: Some(build.to_owned()),
        signature: Some(signature.to_owned()),
        next_attempt_at: existing.map_or(now, |e| e.next_attempt_at),
        ..plan_enqueue(existing, pipeline, email_id, reason, now)
    }
}

/// Marks one due row as attempted before any network or handler work.
pub fn plan_claim(row: &EmailRetryData, now: i64) -> EmailRetryData {
    EmailRetryData {
        attempts: row.attempts + 1,
        next_attempt_at: now + retry_delay_ms(row.attempts + 1),
        ..row.clone()
    }
}

/// Due now, exhausted and parked rows excluded, oldest schedule first.
pub fn select_due(rows: &[EmailRetryData], now: i64) -> Vec<EmailRetryData> {
    let mut due: Vec<EmailRetryData> = rows
        .iter()
        .filter(|r| {
            r.attempts < MAX_RETRY_ATTEMPTS
                && r.next_attempt_at <= now
                && r.awaiting_build.is_none()
        })
        .cloned()
        .collect();
    due.sort_by_key(|r| r.next_attempt_at);
    due
}

/// Enqueues (or coalesces into) a retry for `pipeline`/`email_id`, atomically.
pub async fn enqueue(
    store: &Store,
    pipeline: &str,
    email_id: &str,
    reason: &str,
) -> Result<EmailRetryData, StoreError> {
    let now = store.clock().now_ms();
    let (pipeline, email_id, reason) =
        (pipeline.to_owned(), email_id.to_owned(), reason.to_owned());
    store
        .write(move |tx| {
            let existing = tx.get::<EmailRetryData>(&retry_key(&pipeline, &email_id))?;
            let row = plan_enqueue(existing.as_ref(), &pipeline, &email_id, &reason, now);
            tx.upsert(&row, UpsertOpts::default())?;
            Ok::<_, StoreError>(row)
        })
        .await
}

/// Persists the claimed attempt and returns it.
pub async fn claim(store: &Store, row: &EmailRetryData) -> Result<EmailRetryData, StoreError> {
    let claimed = plan_claim(row, store.clock().now_ms());
    let written = claimed.clone();
    store
        .write(move |tx| tx.upsert(&claimed, UpsertOpts::default()))
        .await?;
    Ok(written)
}

/// Removes the retry; `true` when a row existed.
pub async fn clear(store: &Store, pipeline: &str, email_id: &str) -> Result<bool, StoreError> {
    let key = retry_key(pipeline, email_id);
    store
        .write(move |tx| tx.delete::<EmailRetryData>(&key))
        .await
}

pub async fn get(store: &Store, retry_key: &str) -> Result<Option<EmailRetryData>, StoreError> {
    let key = retry_key.to_owned();
    store
        .read(move |docs| docs.get::<EmailRetryData>(&key))
        .await
}

pub async fn get_all(store: &Store) -> Result<Vec<EmailRetryData>, StoreError> {
    store.read(|docs| docs.get_all::<EmailRetryData>()).await
}

#[cfg(test)]
mod retry_spec {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn make_retry(retry_key: &str, attempts: i64, next_attempt_at: i64) -> EmailRetryData {
        EmailRetryData {
            retry_key: retry_key.to_owned(),
            pipeline: "ParcelTracker".to_owned(),
            email_id: "email-1".to_owned(),
            reason: "Parcel API 503".to_owned(),
            enqueue_count: None,
            attempts,
            next_attempt_at,
            created_at: NOW - 60_000,
            awaiting_build: None,
            signature: None,
            extra: Extra::new(),
        }
    }

    fn keys(rows: &[EmailRetryData]) -> Vec<&str> {
        rows.iter().map(|r| r.retry_key.as_str()).collect()
    }

    #[test]
    fn returns_rows_whose_next_attempt_at_has_passed() {
        let rows = [
            make_retry("a", 1, NOW - 1),
            make_retry("b", 1, NOW),
            make_retry("c", 1, NOW + 1),
        ];
        assert_eq!(keys(&select_due(&rows, NOW)), ["a", "b"]);
    }

    #[test]
    fn excludes_rows_that_exhausted_their_attempts() {
        let rows = [
            make_retry("ok", MAX_RETRY_ATTEMPTS, NOW - 1),
            make_retry("done", MAX_RETRY_ATTEMPTS + 1, NOW - 1),
        ];
        assert!(select_due(&rows, NOW).is_empty());
    }

    #[test]
    fn sorts_due_rows_oldest_first_by_next_attempt_at() {
        let rows = [
            make_retry("later", 1, NOW - 1),
            make_retry("earlier", 1, NOW - 100),
        ];
        assert_eq!(keys(&select_due(&rows, NOW)), ["earlier", "later"]);
    }

    #[test]
    fn coalesces_repeated_signals_without_consuming_retry_attempts() {
        let first = plan_enqueue(None, "ParcelTracker", "email-1", "a", NOW);
        let second = plan_enqueue(Some(&first), "ParcelTracker", "email-1", "b", NOW + 1);
        assert_eq!(second.attempts, 0);
        assert_eq!(second.enqueue_count, Some(2));
        assert_eq!(second.reason, "b");
        assert_eq!(second.next_attempt_at, first.next_attempt_at);
    }

    #[test]
    fn parked_rows_are_never_due() {
        let parked = EmailRetryData {
            awaiting_build: Some("sha256:aaa".to_owned()),
            ..make_retry("parked", 1, NOW - 1)
        };
        assert!(select_due(&[parked], NOW).is_empty());
    }

    #[test]
    fn a_systemic_signal_parks_the_row_and_keeps_its_attempts() {
        let first = plan_enqueue(None, "CalendarEvents", "email-1", "503", NOW);
        let claimed = plan_claim(&first, NOW + 1);
        let parked = plan_enqueue_systemic(
            Some(&claimed),
            "CalendarEvents",
            "email-1",
            "provider error 400",
            "abc",
            "b1",
            NOW + 2,
        );
        assert_eq!(parked.attempts, 1);
        assert_eq!(parked.awaiting_build.as_deref(), Some("b1"));
        assert_eq!(parked.enqueue_count, Some(2));
        let unparked = plan_enqueue(Some(&parked), "CalendarEvents", "email-1", "503", NOW + 3);
        assert_eq!(unparked.awaiting_build, None);
        assert_eq!(unparked.next_attempt_at, NOW + 3 + retry_delay_ms(1));
    }

    #[test]
    fn doubles_the_delay_per_attempt_starting_at_30_minutes() {
        assert_eq!(retry_delay_ms(1), 30 * 60_000);
        assert_eq!(retry_delay_ms(2), 60 * 60_000);
        assert_eq!(retry_delay_ms(3), 120 * 60_000);
    }

    #[test]
    fn treats_attempt_0_like_the_first_attempt() {
        assert_eq!(retry_delay_ms(0), 30 * 60_000);
    }
}
