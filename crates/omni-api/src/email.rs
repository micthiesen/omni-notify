//! Owned by WP02: email activity, activity logs, sender rules and feedback
//! (`src/server.ts` email routes). The string unions are shared with the
//! persisted entities in `omni-email`, which re-exports them.

use serde::{Deserialize, Serialize};

use crate::common::Ms;
use crate::runs::RunLogLine;

/// `EmailPipelineName`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmailPipelineName {
    #[serde(rename = "ParcelTracker")]
    ParcelTracker,
    #[serde(rename = "CalendarEvents")]
    CalendarEvents,
}

impl EmailPipelineName {
    pub const ALL: [EmailPipelineName; 2] = [Self::ParcelTracker, Self::CalendarEvents];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ParcelTracker => "ParcelTracker",
            Self::CalendarEvents => "CalendarEvents",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }
}

impl std::fmt::Display for EmailPipelineName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which filter tier admitted a candidate email (`AdmitTier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdmitTier {
    #[serde(rename = "rule")]
    Rule,
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "triage")]
    Triage,
    #[serde(rename = "keyword-fallback")]
    KeywordFallback,
    /// Parcel only.
    #[serde(rename = "carrier-name")]
    CarrierName,
}

impl AdmitTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Builtin => "builtin",
            Self::Triage => "triage",
            Self::KeywordFallback => "keyword-fallback",
            Self::CarrierName => "carrier-name",
        }
    }
}

/// `EmailActivityOutcome`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmailActivityOutcome {
    /// Did not pass the candidate filter.
    #[serde(rename = "filtered")]
    Filtered,
    /// Legacy only (old whole-email dedup); kept for stored rows.
    #[serde(rename = "skipped")]
    Skipped,
    /// Extraction ran and found nothing actionable.
    #[serde(rename = "no_matches")]
    NoMatches,
    /// Every extracted item succeeded.
    #[serde(rename = "processed")]
    Processed,
    /// Some items succeeded and some failed.
    #[serde(rename = "partial")]
    Partial,
    /// Every extracted item failed.
    #[serde(rename = "failed")]
    Failed,
    /// Processing failed.
    #[serde(rename = "error")]
    Error,
}

impl EmailActivityOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Filtered => "filtered",
            Self::Skipped => "skipped",
            Self::NoMatches => "no_matches",
            Self::Processed => "processed",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Error => "error",
        }
    }
}

/// `EmailRuleScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleScope {
    Parcel,
    Calendar,
    Both,
}

impl RuleScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parcel => "parcel",
            Self::Calendar => "calendar",
            Self::Both => "both",
        }
    }
}

/// `EmailRuleVerdict`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleVerdict {
    Block,
    Allow,
}

impl RuleVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Allow => "allow",
        }
    }
}

/// `EmailFeedbackVerdict`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EmailFeedbackVerdict {
    /// The pipeline processed this email but should not have.
    #[serde(rename = "not_relevant")]
    NotRelevant,
    /// The pipeline filtered this email but should have processed it.
    #[serde(rename = "missed")]
    Missed,
}

/// `serializeEmailActivity()`: optional fields are explicit `null`s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailActivity {
    pub activity_id: String,
    pub pipeline: EmailPipelineName,
    pub email_id: String,
    pub subject: String,
    pub from: String,
    pub received_at: Ms,
    pub processed_at: Ms,
    pub outcome: EmailActivityOutcome,
    pub detail: Option<String>,
    pub admit_reason: Option<String>,
    pub admit_tier: Option<AdmitTier>,
    /// `null` when no priced LLM call is attributable to the row.
    pub cost_cents: Option<f64>,
    pub items: Vec<String>,
}

/// `GET /api/email-activity?pipeline=&limit=`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmailActivitiesResponse {
    pub activities: Vec<EmailActivity>,
}

/// `GET /api/email-activity/:activityId/logs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmailActivityLogsResponse {
    pub activity: EmailActivity,
    pub lines: Vec<RunLogLine>,
    pub dropped: u64,
}

/// `POST /api/email-activity/:activityId/reprocess`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmailActivityResponse {
    pub activity: EmailActivity,
}

/// A stored sender rule (`EmailRuleData`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailRule {
    /// `<scope>:<pattern>`.
    pub rule_id: String,
    pub pattern: String,
    pub scope: RuleScope,
    pub verdict: RuleVerdict,
    pub created_at: Ms,
}

/// One pipeline's read-only built-in lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinSenderLists {
    pub blocked: Vec<String>,
    pub auto_pass: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinRules {
    pub parcel: BuiltinSenderLists,
    pub calendar: BuiltinSenderLists,
}

/// `GET /api/email-rules`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmailRulesResponse {
    /// Newest first.
    pub rules: Vec<EmailRule>,
    pub builtin: BuiltinRules,
}

/// `POST /api/email-rules` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailRuleInput {
    pub pattern: String,
    pub scope: RuleScope,
    pub verdict: RuleVerdict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleUpsertStatus {
    /// 201.
    Created,
    Merged,
    Exists,
    /// A built-in list already blocks every targeted sender; nothing stored.
    Builtin,
}

/// `POST /api/email-rules` response: `{rule, status}`, or
/// `{status: "builtin", message}` when a built-in list already covers it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmailRuleUpsertResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<EmailRule>,
    pub status: RuleUpsertStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `DELETE /api/email-rules/:ruleId` and
/// `DELETE /api/parcel-tracker/deliveries/:trackingNumber`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedResponse {
    pub deleted: bool,
}

/// A stored correction (`EmailFeedbackData`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailFeedback {
    pub activity_id: String,
    pub pipeline: EmailPipelineName,
    pub email_id: String,
    pub subject: String,
    pub from: String,
    pub verdict: EmailFeedbackVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: Ms,
}

/// `POST /api/email-activity/:activityId/feedback` body: `verdict` is required
/// and `null` clears the correction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailFeedbackInput {
    pub verdict: Option<EmailFeedbackVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `POST /api/email-activity/:activityId/feedback` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailFeedbackResponse {
    pub feedback: Option<EmailFeedback>,
}

/// `GET /api/email-feedback`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailFeedbackListResponse {
    pub feedback: Vec<EmailFeedback>,
}

/// Path builders for the WP02 routes.
pub mod paths {
    use crate::common::encode_uri_component;

    pub const EMAIL_ACTIVITY: &str = "/api/email-activity";
    pub const EMAIL_RULES: &str = "/api/email-rules";
    pub const EMAIL_FEEDBACK: &str = "/api/email-feedback";

    /// `/api/email-activity?pipeline=&limit=`.
    pub fn email_activity(
        pipeline: Option<super::EmailPipelineName>,
        limit: Option<u32>,
    ) -> String {
        let mut query = Vec::new();
        if let Some(pipeline) = pipeline {
            query.push(format!("pipeline={}", pipeline.as_str()));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        if query.is_empty() {
            EMAIL_ACTIVITY.to_owned()
        } else {
            format!("{EMAIL_ACTIVITY}?{}", query.join("&"))
        }
    }

    pub fn email_activity_logs(activity_id: &str) -> String {
        format!(
            "{EMAIL_ACTIVITY}/{}/logs",
            encode_uri_component(activity_id)
        )
    }

    pub fn email_activity_reprocess(activity_id: &str) -> String {
        format!(
            "{EMAIL_ACTIVITY}/{}/reprocess",
            encode_uri_component(activity_id)
        )
    }

    pub fn email_activity_feedback(activity_id: &str) -> String {
        format!(
            "{EMAIL_ACTIVITY}/{}/feedback",
            encode_uri_component(activity_id)
        )
    }

    pub fn email_rule(rule_id: &str) -> String {
        format!("{EMAIL_RULES}/{}", encode_uri_component(rule_id))
    }

    pub fn parcel_delivery(tracking_number: &str) -> String {
        format!(
            "/api/parcel-tracker/deliveries/{}",
            encode_uri_component(tracking_number)
        )
    }
}
