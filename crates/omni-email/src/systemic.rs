//! Systemic extraction failures: classification, build-gated replay and one
//! alert per error signature.
//!
//! A provider rejecting the request itself (a non-retryable 4xx), an invalid
//! output schema or a missing API key means the running build or its
//! configuration is broken: every email fails the same way, and retrying on
//! the same build only burns attempts. Such a failure parks the email's
//! `email-retry` row with `awaitingBuild` set to the running build and logs at
//! WARN; [`SystemicReporter`] sends one Pushover per pipeline and signature
//! per [`ALERT_COOLDOWN_MS`] instead of one ERROR alert per email.
//!
//! On boot under a different build, [`release_for_build`] makes at most
//! [`MAX_RELEASES_PER_BOOT`] parked rows due, once per build per email (the
//! durable `email-replay` ledger) and at most [`MAX_BUILD_REPLAYS`] builds per
//! email. It also adopts recent systemic `error` activity rows that have no
//! retry row (failures recorded before this queue existed, or whose transient
//! retries were exhausted). `EmailRetry` replays released rows through the
//! owning handler. Only [`REPLAY_SAFE_PIPELINES`] replay: a systemic failure
//! happens at extraction, before any write, and their handlers dedup every
//! write (created-event hashes, Parcel tracking reservations).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use omni_ai::AiError;
use omni_alerts::{Pushover, PushoverChannel, PushoverMessage};
use omni_core::js::utf16_slice;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use crate::activity::{EmailActivityData, EmailActivityOutcome};
use crate::retry::{self, EmailRetryData, MAX_RETRY_ATTEMPTS};

const LOG: &str = "Main:EmailRetry";

/// Pipelines whose handlers dedup every write, so a replay is safe.
pub const REPLAY_SAFE_PIPELINES: [&str; 2] = ["CalendarEvents", "ParcelTracker"];
/// Parked rows made due per boot; the rest wait for the next boot.
pub const MAX_RELEASES_PER_BOOT: usize = 20;
/// Builds one email is replayed under before it is dropped.
pub const MAX_BUILD_REPLAYS: usize = 3;
/// Failures older than this are not replayed (14 days).
pub const REPLAY_LOOKBACK_MS: i64 = 14 * 24 * 60 * 60_000;
/// One alert per pipeline and signature per day.
pub const ALERT_COOLDOWN_MS: i64 = 24 * 60 * 60_000;
/// Ledger and alert rows idle this long are pruned at boot (30 days).
const PRUNE_AFTER_MS: i64 = 30 * 24 * 60 * 60_000;
const MAX_SUMMARY_CHARS: usize = 200;
const MAX_REASON_CHARS: usize = 2_000;

/// A normalized error text and its short stable key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// 12 hex characters of the summary's SHA-256.
    pub key: String,
    /// Whitespace-collapsed text with request ids and long numbers replaced by `#`.
    pub summary: String,
}

#[allow(clippy::expect_used)]
static VOLATILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:req|call|resp|msg|run)_[A-Za-z0-9]+\b|\b[0-9a-fA-F]{8,}(?:-[0-9a-fA-F]{4,})*\b|\b\d{4,}\b",
    )
    .expect("static regex")
});
#[allow(clippy::expect_used)]
static RECORDED_PROVIDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)provider error (\d{3}): (.*)").expect("static regex"));
#[allow(clippy::expect_used)]
static RECORDED_SCHEMA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)invalid output schema: .*").expect("static regex"));
#[allow(clippy::expect_used)]
static RECORDED_MISSING_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"missing API key for \w+").expect("static regex"));

impl Signature {
    pub fn from_text(text: &str) -> Self {
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let replaced = VOLATILE.replace_all(&collapsed, "#");
        let summary = utf16_slice(&replaced, 0, MAX_SUMMARY_CHARS).into_owned();
        let mut key = omni_core::digest::sha256_hex(summary.as_bytes());
        key.truncate(12);
        Self { key, summary }
    }

    fn provider(status: u16, message: &str) -> Self {
        Self::from_text(&format!("provider error {status}: {message}"))
    }
}

/// How an extraction failure should be handled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureClass {
    /// Network, timeout, 408/409/429 or 5xx: the ordinary retry queue.
    Transient,
    /// The model's output was rejected for this email.
    Content,
    /// The request itself was rejected: the build or configuration is broken.
    Systemic(Signature),
}

fn is_systemic_status(status: u16) -> bool {
    (400..500).contains(&status) && !matches!(status, 408 | 409 | 429)
}

/// Classifies a model call failure.
pub fn classify_ai_error(error: &AiError) -> FailureClass {
    match error {
        AiError::Provider { status, message } if is_systemic_status(*status) => {
            FailureClass::Systemic(Signature::provider(*status, message))
        }
        AiError::Timeout | AiError::Http(_) | AiError::Provider { .. } => FailureClass::Transient,
        AiError::Schema(message) if message.starts_with("No object generated") => {
            FailureClass::Content
        }
        AiError::Schema(message) if message.starts_with("invalid provider response JSON") => {
            FailureClass::Transient
        }
        AiError::Schema(message) => FailureClass::Systemic(Signature::from_text(message)),
        AiError::Refused(_) | AiError::StepLimit => FailureClass::Content,
        AiError::MissingKey(_) => FailureClass::Systemic(Signature::from_text(&error.to_string())),
    }
}

/// The signature of a systemic failure recorded as text (an activity detail or
/// a retry reason), matching what [`classify_ai_error`] gives the same error.
pub fn classify_recorded(text: &str) -> Option<Signature> {
    if let Some(captures) = RECORDED_PROVIDER.captures(text) {
        let status: u16 = captures.get(1)?.as_str().parse().ok()?;
        return is_systemic_status(status)
            .then(|| Signature::provider(status, captures.get(2).map_or("", |m| m.as_str())));
    }
    RECORDED_SCHEMA
        .find(text)
        .or_else(|| RECORDED_MISSING_KEY.find(text))
        .map(|m| Signature::from_text(m.as_str()))
}

pub fn is_replay_safe(pipeline: &str) -> bool {
    REPLAY_SAFE_PIPELINES.contains(&pipeline)
}

static CURRENT_BUILD: OnceCell<String> = OnceCell::const_new();

/// The running build: `sha256:<12 hex>` of the executable, or `unknown`.
pub async fn current_build() -> String {
    CURRENT_BUILD
        .get_or_init(|| async {
            tokio::task::spawn_blocking(executable_digest)
                .await
                .ok()
                .flatten()
                .map_or_else(|| "unknown".to_owned(), |hex| format!("sha256:{hex}"))
        })
        .await
        .clone()
}

fn executable_digest() -> Option<String> {
    use sha2::{Digest as _, Sha256};
    use std::io::Read as _;
    let path = std::env::current_exe().ok()?;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    hex.truncate(12);
    Some(hex)
}

/// `EmailReplayData` (entity `email-replay`, key `retryKey`): the builds an
/// email was released for replay under.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailReplayData {
    pub retry_key: String,
    pub pipeline: String,
    pub email_id: String,
    pub signature: String,
    pub builds: Vec<String>,
    pub last_released_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailReplayData {
    const NAME: &'static str = "email-replay";
    type Key = String;
    fn key(&self) -> String {
        self.retry_key.clone()
    }
}

/// `EmailSystemicAlertData` (entity `email-systemic-alert`, key `alertKey`):
/// one row per pipeline and signature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSystemicAlertData {
    /// `<pipeline>#<signature key>`.
    pub alert_key: String,
    pub pipeline: String,
    pub signature: String,
    pub summary: String,
    pub build: String,
    pub failures: i64,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    /// Reserved before the Pushover is sent.
    pub last_alerted_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailSystemicAlertData {
    const NAME: &'static str = "email-systemic-alert";
    type Key = String;
    fn key(&self) -> String {
        self.alert_key.clone()
    }
}

/// Counts the failure; `true` when this call reserved the alert.
pub fn plan_alert(
    existing: Option<&EmailSystemicAlertData>,
    pipeline: &str,
    signature: &Signature,
    build: &str,
    now: i64,
) -> (EmailSystemicAlertData, bool) {
    let due = existing.is_none_or(|row| now - row.last_alerted_at >= ALERT_COOLDOWN_MS);
    let row = EmailSystemicAlertData {
        alert_key: format!("{pipeline}#{}", signature.key),
        pipeline: pipeline.to_owned(),
        signature: signature.key.clone(),
        summary: signature.summary.clone(),
        build: build.to_owned(),
        failures: existing.map_or(0, |row| row.failures) + 1,
        first_seen_at: existing.map_or(now, |row| row.first_seen_at),
        last_seen_at: now,
        last_alerted_at: if due {
            now
        } else {
            existing.map_or(now, |row| row.last_alerted_at)
        },
        extra: existing.map(|row| row.extra.clone()).unwrap_or_default(),
    };
    (row, due)
}

/// Parks emails that failed systemically and sends the per-signature alert.
#[derive(Clone)]
pub struct SystemicReporter {
    store: Store,
    pushover: Pushover,
}

impl SystemicReporter {
    pub fn new(store: Store, pushover: Pushover) -> Self {
        Self { store, pushover }
    }

    /// Parks `email_id` until a different build runs and, at most once per
    /// [`ALERT_COOLDOWN_MS`] per pipeline and signature, sends one Pushover.
    /// The alert is reserved in the same transaction as the retry row, before
    /// the send, and a failed send is not retried.
    pub async fn report(
        &self,
        pipeline: &str,
        email_id: &str,
        reason: &str,
        signature: &Signature,
    ) -> Result<(), StoreError> {
        let build = current_build().await;
        let now = self.store.clock().now_ms();
        let alert = {
            let (pipeline, email_id, signature, build) = (
                pipeline.to_owned(),
                email_id.to_owned(),
                signature.clone(),
                build.clone(),
            );
            let reason = utf16_slice(reason, 0, MAX_REASON_CHARS).into_owned();
            self.store
                .write(move |tx| {
                    let existing =
                        tx.get::<EmailRetryData>(&retry::retry_key(&pipeline, &email_id))?;
                    let row = retry::plan_enqueue_systemic(
                        existing.as_ref(),
                        &pipeline,
                        &email_id,
                        &reason,
                        &signature.key,
                        &build,
                        now,
                    );
                    tx.upsert(&row, UpsertOpts::default())?;
                    let key = format!("{pipeline}#{}", signature.key);
                    let existing = tx.get::<EmailSystemicAlertData>(&key)?;
                    let (alert, due) =
                        plan_alert(existing.as_ref(), &pipeline, &signature, &build, now);
                    tx.upsert(&alert, UpsertOpts::default())?;
                    Ok::<_, StoreError>(due.then_some(alert))
                })
                .await?
        };
        tracing::warn!(
            target: LOG,
            "{pipeline} email {email_id} parked until a new build: {} [{}]",
            signature.summary,
            signature.key
        );
        if let Some(alert) = alert {
            self.send_alert(&alert).await;
        }
        Ok(())
    }

    async fn send_alert(&self, alert: &EmailSystemicAlertData) {
        let label = match alert.pipeline.as_str() {
            "CalendarEvents" => "Calendar extraction",
            "ParcelTracker" => "Parcel extraction",
            other => other,
        };
        let message = PushoverMessage {
            title: Some(format!("{label} failing")),
            message: format!(
                "Emails fail with the same error: {}\nThey replay once a new build is running.",
                alert.summary
            ),
            ..PushoverMessage::default()
        };
        if let Err(error) = self.pushover.send(PushoverChannel::General, message).await {
            tracing::warn!(target: LOG, "Could not send the {label} failure alert: {error}");
        }
    }
}

/// What one boot release does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReleasePlan {
    /// Retry rows to write, due now.
    pub release: Vec<EmailRetryData>,
    /// Ledger rows to write, one per released row.
    pub ledger: Vec<EmailReplayData>,
    /// Parked retry rows to drop (too old or replayed under enough builds).
    pub expire: Vec<String>,
    /// Ledger rows to prune.
    pub prune_ledger: Vec<String>,
    /// Eligible rows left for a later boot by the cap.
    pub deferred: usize,
}

struct ReleaseCandidate {
    row: EmailRetryData,
    signature: String,
}

/// Plans one boot's release (pure): parked rows from another build, legacy
/// retry rows whose reason is systemic, and recent systemic `error` activity
/// without a retry row, newest first, at most `cap`.
pub fn plan_release(
    retries: &[EmailRetryData],
    ledger: &[EmailReplayData],
    activity: &[EmailActivityData],
    build: &str,
    now: i64,
    cap: usize,
) -> ReleasePlan {
    let ledger_by_key: HashMap<&str, &EmailReplayData> = ledger
        .iter()
        .map(|row| (row.retry_key.as_str(), row))
        .collect();
    let retry_keys: HashSet<&str> = retries.iter().map(|r| r.retry_key.as_str()).collect();
    let cutoff = now - REPLAY_LOOKBACK_MS;
    let mut plan = ReleasePlan::default();
    let mut candidates = Vec::new();

    for row in retries {
        if !is_replay_safe(&row.pipeline) {
            continue;
        }
        let parked = row.awaiting_build.is_some();
        if parked && row.created_at < cutoff {
            // Too old to replay under any build, including this one.
            plan.expire.push(row.retry_key.clone());
            continue;
        }
        if row.awaiting_build.as_deref() == Some(build) {
            continue;
        }
        let Some(signature) = row
            .signature
            .clone()
            .or_else(|| classify_recorded(&row.reason).map(|s| s.key))
        else {
            continue;
        };
        let replays = ledger_by_key.get(row.retry_key.as_str());
        if replays.is_some_and(|l| l.builds.iter().any(|b| b == build)) {
            continue;
        }
        let exhausted = replays.is_some_and(|l| l.builds.len() >= MAX_BUILD_REPLAYS)
            || row.attempts >= MAX_RETRY_ATTEMPTS;
        if exhausted || row.created_at < cutoff {
            if parked {
                plan.expire.push(row.retry_key.clone());
            }
            continue;
        }
        candidates.push(ReleaseCandidate {
            row: EmailRetryData {
                awaiting_build: None,
                signature: Some(signature.clone()),
                next_attempt_at: now,
                ..row.clone()
            },
            signature,
        });
    }

    for row in activity {
        let pipeline = row.pipeline.as_str();
        if row.outcome != EmailActivityOutcome::Error
            || !is_replay_safe(pipeline)
            || row.processed_at < cutoff
            || retry_keys.contains(row.activity_id.as_str())
            || ledger_by_key.contains_key(row.activity_id.as_str())
        {
            continue;
        }
        let Some(detail) = row.detail.as_deref() else {
            continue;
        };
        let Some(signature) = classify_recorded(detail) else {
            continue;
        };
        candidates.push(ReleaseCandidate {
            row: EmailRetryData {
                retry_key: retry::retry_key(pipeline, &row.email_id),
                pipeline: pipeline.to_owned(),
                email_id: row.email_id.clone(),
                reason: utf16_slice(detail, 0, MAX_REASON_CHARS).into_owned(),
                enqueue_count: Some(1),
                attempts: 0,
                next_attempt_at: now,
                created_at: row.processed_at,
                awaiting_build: None,
                signature: Some(signature.key.clone()),
                extra: Extra::new(),
            },
            signature: signature.key,
        });
    }

    candidates.sort_by_key(|c| std::cmp::Reverse(c.row.created_at));
    plan.deferred = candidates.len().saturating_sub(cap);
    for candidate in candidates.into_iter().take(cap) {
        let key = candidate.row.retry_key.clone();
        let mut ledger_row = ledger_by_key
            .get(key.as_str())
            .map(|row| (*row).clone())
            .unwrap_or_else(|| EmailReplayData {
                retry_key: key.clone(),
                pipeline: candidate.row.pipeline.clone(),
                email_id: candidate.row.email_id.clone(),
                signature: candidate.signature.clone(),
                builds: Vec::new(),
                last_released_at: now,
                extra: Extra::new(),
            });
        ledger_row.builds.push(build.to_owned());
        ledger_row.signature = candidate.signature;
        ledger_row.last_released_at = now;
        plan.ledger.push(ledger_row);
        plan.release.push(candidate.row);
    }

    plan.prune_ledger = ledger
        .iter()
        .filter(|row| {
            row.last_released_at < now - PRUNE_AFTER_MS
                && !retry_keys.contains(row.retry_key.as_str())
        })
        .map(|row| row.retry_key.clone())
        .collect();
    plan
}

/// Counts from one [`release_for_build`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReleaseReport {
    pub released: usize,
    pub deferred: usize,
    pub expired: usize,
}

/// Plans and applies one boot's release in one transaction, and prunes idle
/// alert rows.
pub async fn release_for_build(
    store: &Store,
    build: &str,
    cap: usize,
) -> Result<ReleaseReport, StoreError> {
    let now = store.clock().now_ms();
    let build = build.to_owned();
    store
        .write(move |tx| {
            let retries = tx.get_all::<EmailRetryData>()?;
            let ledger = tx.get_all::<EmailReplayData>()?;
            let activity = tx.get_all::<EmailActivityData>()?;
            let plan = plan_release(&retries, &ledger, &activity, &build, now, cap);
            for row in &plan.release {
                tx.upsert(row, UpsertOpts::default())?;
            }
            for row in &plan.ledger {
                tx.upsert(row, UpsertOpts::default())?;
            }
            for key in &plan.expire {
                tx.delete::<EmailRetryData>(key)?;
            }
            for key in &plan.prune_ledger {
                tx.delete::<EmailReplayData>(key)?;
            }
            for alert in tx.get_all::<EmailSystemicAlertData>()? {
                if alert.last_seen_at < now - PRUNE_AFTER_MS {
                    tx.delete::<EmailSystemicAlertData>(&alert.alert_key)?;
                }
            }
            Ok::<_, StoreError>(ReleaseReport {
                released: plan.release.len(),
                deferred: plan.deferred,
                expired: plan.expire.len(),
            })
        })
        .await
}

pub async fn alerts(store: &Store) -> Result<Vec<EmailSystemicAlertData>, StoreError> {
    store
        .read(|docs| docs.get_all::<EmailSystemicAlertData>())
        .await
}

#[cfg(test)]
mod systemic_spec {
    use super::*;

    const AUGUST_REQUIRED: &str = "Invalid schema for response_format 'calendar_event_extraction': In context=(), 'required' is required to be supplied and to be an array including every key in properties. Missing 'location'.";
    const OCTOBER_REF: &str = "Invalid schema for response_format 'calendar_event_extraction': In context=('properties', 'events', 'items'), $ref cannot have keywords {'description'}.";

    fn provider(status: u16, message: &str) -> AiError {
        AiError::Provider {
            status,
            message: message.to_owned(),
        }
    }

    fn systemic(class: FailureClass) -> Signature {
        match class {
            FailureClass::Systemic(signature) => signature,
            other => panic!("expected systemic, got {other:?}"),
        }
    }

    #[test]
    fn schema_rejections_by_the_provider_are_systemic() {
        let october = systemic(classify_ai_error(&provider(400, OCTOBER_REF)));
        let august = systemic(classify_ai_error(&provider(400, AUGUST_REQUIRED)));
        assert_ne!(october.key, august.key);
        assert!(
            october
                .summary
                .starts_with("provider error 400: Invalid schema")
        );
        assert_eq!(october.key.len(), 12);
    }

    #[test]
    fn rate_limits_server_errors_and_timeouts_are_transient() {
        assert_eq!(
            classify_ai_error(&provider(429, "Rate limit reached")),
            FailureClass::Transient
        );
        assert_eq!(
            classify_ai_error(&provider(503, "overloaded")),
            FailureClass::Transient
        );
        assert_eq!(
            classify_ai_error(&AiError::Timeout),
            FailureClass::Transient
        );
        assert_eq!(
            classify_ai_error(&AiError::Schema(
                "invalid provider response JSON: EOF".to_owned()
            )),
            FailureClass::Transient
        );
    }

    #[test]
    fn rejected_model_output_is_a_content_failure() {
        assert_eq!(
            classify_ai_error(&AiError::Schema(
                omni_ai::schema::SCHEMA_MISMATCH.to_owned()
            )),
            FailureClass::Content
        );
        assert_eq!(
            classify_ai_error(&AiError::Refused("no".to_owned())),
            FailureClass::Content
        );
    }

    #[test]
    fn local_schema_and_key_errors_are_systemic() {
        let schema = AiError::Schema("invalid output schema: bad $ref".to_owned());
        assert!(matches!(
            classify_ai_error(&schema),
            FailureClass::Systemic(_)
        ));
        let key = AiError::MissingKey(omni_ai::Provider::OpenAi);
        assert!(matches!(classify_ai_error(&key), FailureClass::Systemic(_)));
    }

    #[test]
    fn recorded_text_yields_the_same_signature_as_the_live_error() {
        let live = systemic(classify_ai_error(&provider(400, OCTOBER_REF)));
        let calendar_detail = format!(
            "extraction failed: Calendar extraction failed: provider error 400: {OCTOBER_REF}"
        );
        assert_eq!(classify_recorded(&calendar_detail), Some(live));

        let schema = systemic(classify_ai_error(&AiError::Schema(
            "invalid output schema: bad".to_owned(),
        )));
        assert_eq!(
            classify_recorded("Parcel extraction failed: invalid output schema: bad"),
            Some(schema)
        );
    }

    #[test]
    fn recorded_transient_and_content_text_is_not_systemic() {
        assert_eq!(
            classify_recorded("Calendar extraction failed: provider error 503: busy"),
            None
        );
        assert_eq!(
            classify_recorded("Calendar extraction failed: provider error 429: slow down"),
            None
        );
        assert_eq!(
            classify_recorded(
                "Calendar extraction failed: No object generated: response did not match schema."
            ),
            None
        );
        assert_eq!(
            classify_recorded("calendar discovery failed: timeout"),
            None
        );
    }

    #[test]
    fn request_ids_and_long_numbers_do_not_split_a_signature() {
        let a = Signature::from_text("provider error 400: bad request req_abc123 at 1791590400");
        let b = Signature::from_text("provider error 400:  bad request req_zz9 at 1791590999");
        assert_eq!(a, b);
        assert!(a.summary.contains("400"));
    }

    #[test]
    fn the_first_failure_reserves_the_alert_and_repeats_within_a_day_do_not() {
        let signature = Signature::from_text("provider error 400: x");
        let (first, due) = plan_alert(None, "CalendarEvents", &signature, "b1", 1_000);
        assert!(due);
        let (second, due) = plan_alert(Some(&first), "CalendarEvents", &signature, "b1", 2_000);
        assert!(!due);
        assert_eq!(second.failures, 2);
        assert_eq!(second.last_alerted_at, 1_000);
        let (_, due) = plan_alert(
            Some(&second),
            "CalendarEvents",
            &signature,
            "b1",
            1_000 + ALERT_COOLDOWN_MS,
        );
        assert!(due);
    }
}
