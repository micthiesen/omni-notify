//! Explicit user corrections on pipeline outcomes,
//! injected into the triage prompt.

use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

use crate::activity::{EmailPipelineName, activity_id};
use crate::sender_rules::RuleTarget;

pub use omni_api::email::EmailFeedbackVerdict;

/// `EmailFeedbackData` (entity `email-feedback`, key `activityId`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailFeedbackData {
    /// `<pipeline>#<emailId>`, matching the activity row.
    pub activity_id: String,
    pub pipeline: EmailPipelineName,
    pub email_id: String,
    pub subject: String,
    pub from: String,
    pub verdict: EmailFeedbackVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailFeedbackData {
    const NAME: &'static str = "email-feedback";
    type Key = String;
    fn key(&self) -> String {
        self.activity_id.clone()
    }
}

/// One `recordEmailFeedback` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFeedback {
    pub pipeline: EmailPipelineName,
    pub email_id: String,
    pub subject: String,
    pub from: String,
    pub verdict: EmailFeedbackVerdict,
    pub note: Option<String>,
}

/// `recordEmailFeedback`: upserts (re-recording the same email overwrites).
pub async fn record(store: &Store, entry: NewFeedback) -> Result<EmailFeedbackData, StoreError> {
    let row = EmailFeedbackData {
        activity_id: activity_id(entry.pipeline, &entry.email_id),
        pipeline: entry.pipeline,
        email_id: entry.email_id,
        subject: entry.subject,
        from: entry.from,
        verdict: entry.verdict,
        note: entry.note,
        created_at: store.clock().now_ms(),
        extra: Extra::new(),
    };
    let written = row.clone();
    store
        .write(move |tx| tx.upsert(&row, UpsertOpts::default()))
        .await?;
    Ok(written)
}

/// `deleteEmailFeedback`: `true` when the row existed.
pub async fn delete(store: &Store, activity_id: &str) -> Result<bool, StoreError> {
    let key = activity_id.to_owned();
    store
        .write(move |tx| tx.delete::<EmailFeedbackData>(&key))
        .await
}

/// `listEmailFeedback`: newest first, optionally one pipeline, capped.
pub async fn list(
    store: &Store,
    pipeline: Option<EmailPipelineName>,
    limit: usize,
) -> Result<Vec<EmailFeedbackData>, StoreError> {
    let mut rows: Vec<EmailFeedbackData> = store
        .read(|docs| docs.get_all::<EmailFeedbackData>())
        .await?
        .into_iter()
        .filter(|f| pipeline.is_none_or(|p| f.pipeline == p))
        .collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.created_at));
    rows.truncate(limit);
    Ok(rows)
}

/// Default digest size used by the triage prompt.
pub const DIGEST_LIMIT: usize = 15;

fn pipeline_for(target: RuleTarget) -> EmailPipelineName {
    match target {
        RuleTarget::Parcel => EmailPipelineName::ParcelTracker,
        RuleTarget::Calendar => EmailPipelineName::CalendarEvents,
    }
}

/// Pure formatting of [`format_digest`].
pub fn digest_lines(rows: &[EmailFeedbackData]) -> String {
    rows.iter()
        .map(|f| {
            let label = match f.verdict {
                EmailFeedbackVerdict::NotRelevant => "user marked NOT relevant",
                EmailFeedbackVerdict::Missed => {
                    "user marked as MISSED (should have been processed)"
                }
            };
            let note = match f.note.as_deref() {
                Some(note) if !note.is_empty() => format!(" (note: {note})"),
                _ => String::new(),
            };
            format!("- \"{}\" from {}: {label}{note}", f.subject, f.from)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `formatFeedbackDigest`: compact correction lines for prompt injection;
/// empty when the pipeline has no feedback.
pub async fn format_digest(
    store: &Store,
    target: RuleTarget,
    limit: usize,
) -> Result<String, StoreError> {
    let rows = list(store, Some(pipeline_for(target)), limit).await?;
    Ok(digest_lines(&rows))
}
