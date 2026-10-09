//! What the calendar pipeline needs from the email pipeline core and the
//! mail transport.
//!
//! The pipeline is written against these traits so tests can record what it
//! does. [`crate::email_core::OmniEmailSupport`] implements [`EmailSupport`]
//! with the `omni_email` library (activity, retry, sender rules, triage,
//! activity log capture); the binary supplies an [`AttachmentSource`] backed by
//! the mail transport.

use futures::future::BoxFuture;
use omni_core::email::{EmailAttachment, FetchedEmail};
use serde::{Deserialize, Serialize};

use crate::pipeline::PipelineError;

/// The calendar pipeline's name in activity rows, retry rows and handler lists.
pub const PIPELINE: &str = "CalendarEvents";

/// A seam call failed.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SupportError {
    pub message: String,
}

impl SupportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Which filter tier admitted a candidate (`AdmitTier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdmitTier {
    #[serde(rename = "rule")]
    Rule,
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "triage")]
    Triage,
    #[serde(rename = "keyword-fallback")]
    KeywordFallback,
}

/// `EmailActivityOutcome` values the calendar pipeline writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOutcome {
    Filtered,
    NoMatches,
    Processed,
    Partial,
    Failed,
    Error,
}

/// Empty → `no_matches`, all ok → `processed`,
/// none ok → `failed`, else `partial`.
pub fn derive_items_outcome(items_ok: &[bool]) -> ActivityOutcome {
    if items_ok.is_empty() {
        return ActivityOutcome::NoMatches;
    }
    let succeeded = items_ok.iter().filter(|ok| **ok).count();
    if succeeded == items_ok.len() {
        ActivityOutcome::Processed
    } else if succeeded == 0 {
        ActivityOutcome::Failed
    } else {
        ActivityOutcome::Partial
    }
}

/// An attributable LLM cost: `None` means no
/// call is attributable, `Some(None)` a call on an unpriced model.
pub type CostCents = Option<Option<f64>>;

/// Absent parts drop out; any unpriced part makes the total
/// unpriced; all absent stays absent.
pub fn sum_cost_cents(parts: &[CostCents]) -> CostCents {
    let known: Vec<Option<f64>> = parts.iter().filter_map(|p| *p).collect();
    if known.is_empty() {
        return None;
    }
    if known.iter().any(Option::is_none) {
        return Some(None);
    }
    Some(Some(known.iter().flatten().sum()))
}

/// One `recordEmailActivity` call.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEntry {
    pub pipeline: &'static str,
    pub email_id: String,
    pub subject: String,
    pub from: String,
    pub received_at: String,
    pub outcome: ActivityOutcome,
    pub detail: Option<String>,
    pub admit_reason: Option<String>,
    pub admit_tier: Option<AdmitTier>,
    pub cost_cents: CostCents,
    pub items: Option<Vec<String>>,
}

impl ActivityEntry {
    pub fn new(email: &FetchedEmail, outcome: ActivityOutcome) -> Self {
        Self {
            pipeline: PIPELINE,
            email_id: email.id.clone(),
            subject: email.subject.clone(),
            from: email.from.clone(),
            received_at: email.received_at.clone(),
            outcome,
            detail: None,
            admit_reason: None,
            admit_tier: None,
            cost_cents: None,
            items: None,
        }
    }
}

/// A user sender rule verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleVerdict {
    Allow,
    Block,
}

/// The matching user rule for the calendar scope (`findSenderRule(from, "calendar")`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SenderRuleMatch {
    pub pattern: String,
    pub verdict: RuleVerdict,
}

/// The shared triage model's verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriageVerdict {
    pub parcel: bool,
    pub calendar: bool,
    pub reason: String,
}

/// Work run under per-email log capture (the email's processing phase).
pub type CapturedWork<'a> = BoxFuture<'a, Result<(), PipelineError>>;

/// The email pipeline core (`omni_email`).
pub trait EmailSupport: Send + Sync {
    /// `findSenderRule(from, "calendar")`.
    fn find_sender_rule<'a>(
        &'a self,
        from: &'a str,
    ) -> BoxFuture<'a, Result<Option<SenderRuleMatch>, SupportError>>;
    /// `EmailTriageService.classify` (memoized per email id, shared with parcel).
    fn classify<'a>(
        &'a self,
        email: &'a FetchedEmail,
    ) -> BoxFuture<'a, Result<TriageVerdict, SupportError>>;
    /// The triage cost of `email_id`: `None` when unknown or unpriced.
    fn triage_cost_cents(&self, email_id: &str) -> Option<f64>;
    /// Records an email activity (upsert + per-pipeline prune).
    fn record_activity(&self, entry: ActivityEntry) -> BoxFuture<'_, Result<(), SupportError>>;
    /// `EmailRetryPersistence.enqueue`.
    fn enqueue_retry<'a>(
        &'a self,
        pipeline: &'static str,
        email_id: &'a str,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<(), SupportError>>;
    /// `withEmailLogCaptureEffect(activityId, pipeline, work)`: runs `work` with
    /// its log lines attributed to the email's activity row, persists them, and
    /// returns the work's own result unchanged.
    fn with_log_capture<'a>(
        &'a self,
        activity_id: String,
        pipeline: &'static str,
        work: CapturedWork<'a>,
    ) -> CapturedWork<'a>;
}

/// A downloaded attachment (`DownloadedAttachment`).
pub use omni_core::email::DownloadedAttachment;

/// The mail transport's attachment download.
pub trait AttachmentSource: Send + Sync {
    fn download<'a>(
        &'a self,
        attachment: &'a EmailAttachment,
    ) -> BoxFuture<'a, Result<Option<DownloadedAttachment>, SupportError>>;
}

/// Production [`AttachmentSource`]: the `EmailReader` port, looked up per call
/// (ports are set after subsystems are built). An unset port fails the
/// download, which the pipeline warns about and skips.
pub struct PortAttachments {
    ports: omni_runtime::Ports,
}

impl PortAttachments {
    pub fn new(ports: omni_runtime::Ports) -> Self {
        Self { ports }
    }
}

impl AttachmentSource for PortAttachments {
    fn download<'a>(
        &'a self,
        attachment: &'a EmailAttachment,
    ) -> BoxFuture<'a, Result<Option<DownloadedAttachment>, SupportError>> {
        Box::pin(async move {
            let reader = self
                .ports
                .email_reader()
                .ok_or_else(|| SupportError::new("EmailReader is unavailable"))?;
            reader
                .download_attachment(attachment)
                .await
                .map_err(|e| SupportError::new(e.to_string()))
        })
    }
}
