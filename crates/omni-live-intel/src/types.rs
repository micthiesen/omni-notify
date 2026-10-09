//! Persisted livestream-intelligence documents.
//!
//! Field order matches the stored rows, so new rows encode like existing ones.
//! Every struct keeps unknown fields in `extra` so read-modify-write
//! never drops data. An optional field is an omitted `Option`; a nullable field
//! is an `Option` that serializes `null`; an optional nullable field is a
//! [`Nullable`].

use indexmap::IndexMap;
use omni_store::cbor::{Extra, JsValue};
use omni_store::entity::Entity;
use serde::{Deserialize, Serialize};

/// `field?: T | null`: `None` is absent, `Some(None)` is `null`.
pub type Nullable<T> = Option<Option<T>>;

/// Metric values (`number | string | boolean | null`), kept as JS values.
pub type Metrics = IndexMap<String, JsValue>;

/// A number metric.
pub fn metric_number(value: f64) -> JsValue {
    JsValue::Float(value)
}

/// A string metric.
pub fn metric_string(value: impl Into<String>) -> JsValue {
    JsValue::String(value.into())
}

/// A nullable number metric.
pub fn metric_nullable(value: Option<f64>) -> JsValue {
    value.map_or(JsValue::Null, JsValue::Float)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LivestreamAlertType {
    #[serde(rename = "destiny_guest")]
    DestinyGuest,
    #[serde(rename = "breaking_news")]
    BreakingNews,
    #[serde(rename = "debate")]
    Debate,
    #[serde(rename = "guest_joined")]
    GuestJoined,
    #[serde(rename = "major_announcement")]
    MajorAnnouncement,
    #[serde(rename = "viewer_surge")]
    ViewerSurge,
    #[serde(rename = "cross_stream_topic")]
    CrossStreamTopic,
}

impl LivestreamAlertType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DestinyGuest => "destiny_guest",
            Self::BreakingNews => "breaking_news",
            Self::Debate => "debate",
            Self::GuestJoined => "guest_joined",
            Self::MajorAnnouncement => "major_announcement",
            Self::ViewerSurge => "viewer_surge",
            Self::CrossStreamTopic => "cross_stream_topic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LivestreamFeedbackVerdict {
    #[serde(rename = "useful")]
    Useful,
    #[serde(rename = "not_useful")]
    NotUseful,
    #[serde(rename = "false_positive")]
    FalsePositive,
}

impl LivestreamFeedbackVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Useful => "useful",
            Self::NotUseful => "not_useful",
            Self::FalsePositive => "false_positive",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "useful" => Some(Self::Useful),
            "not_useful" => Some(Self::NotUseful),
            "false_positive" => Some(Self::FalsePositive),
            _ => None,
        }
    }
}

/// Legacy title classification (no longer produced; cleared on every observation).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticMetadata {
    pub headline: String,
    pub topics: Vec<String>,
    pub content_kind: String,
    pub importance: f64,
    pub reason: String,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerTrend {
    pub percent_change: f64,
    pub viewers_per_minute: f64,
    pub dgg_percent_change: Option<f64>,
    pub anomalous: bool,
    pub reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub current_viewers: Nullable<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub baseline_viewers: Nullable<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub current_dgg_viewers: Nullable<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub baseline_dgg_viewers: Nullable<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_samples: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub typical_peak_viewers: Nullable<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_observations: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_with::rust::double_option"
    )]
    pub suppression_reason: Nullable<String>,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamChapter {
    pub chapter_id: String,
    /// `now - durationSeconds * 1000`, which can be fractional.
    pub started_at: f64,
    pub title: String,
    pub summary: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollingSummary {
    pub text: String,
    pub topic: String,
    pub confidence: f64,
    pub transcript_excerpt: String,
    pub updated_at: i64,
    pub window_seconds: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresenceState {
    #[serde(rename = "possible")]
    Possible,
    #[serde(rename = "confirmed")]
    Confirmed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinyPresence {
    pub state: PresenceState,
    pub confidence: f64,
    pub detected_at: i64,
    pub reason: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamAlertRecord {
    pub alert_id: String,
    #[serde(rename = "type")]
    pub alert_type: LivestreamAlertType,
    pub title: String,
    pub message: String,
    pub reason: String,
    pub confidence: f64,
    pub created_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `livestream-intelligence`, keyed by `streamerId`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamIntelligenceData {
    pub streamer_id: String,
    pub session_started_at: i64,
    pub relevance_score: f64,
    pub relevance_reasons: Vec<String>,
    pub chapters: Vec<LivestreamChapter>,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<SemanticMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend: Option<ViewerTrend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RollingSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destiny_presence: Option<DestinyPresence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_alert: Option<LivestreamAlertRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerted_at_by_type: Option<IndexMap<String, i64>>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for LivestreamIntelligenceData {
    const NAME: &'static str = "livestream-intelligence";
    type Key = String;
    fn key(&self) -> String {
        self.streamer_id.clone()
    }
}

/// `livestream-feedback`, keyed by `feedbackId` (= the alert id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamFeedbackData {
    pub feedback_id: String,
    pub streamer_id: String,
    pub alert_id: String,
    pub alert_type: LivestreamAlertType,
    pub verdict: LivestreamFeedbackVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for LivestreamFeedbackData {
    const NAME: &'static str = "livestream-feedback";
    type Key = String;
    fn key(&self) -> String {
        self.feedback_id.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PipelineStage {
    #[serde(rename = "metadata")]
    Metadata,
    #[serde(rename = "voice")]
    Voice,
    #[serde(rename = "summary")]
    Summary,
    #[serde(rename = "alert")]
    Alert,
}

impl PipelineStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Voice => "voice",
            Self::Summary => "summary",
            Self::Alert => "alert",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "metadata" => Some(Self::Metadata),
            "voice" => Some(Self::Voice),
            "summary" => Some(Self::Summary),
            "alert" => Some(Self::Alert),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "error")]
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageDiagnostic {
    pub status: PipelineStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eligible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl StageDiagnostic {
    /// A diagnostic with only a status.
    pub fn new(status: PipelineStatus) -> Self {
        Self {
            status,
            eligible: None,
            started_at: None,
            finished_at: None,
            next_at: None,
            duration_ms: None,
            detail: None,
            metrics: None,
            extra: Extra::new(),
        }
    }
}

/// `livestream-diagnostics`, keyed by `streamerId`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamDiagnosticsData {
    pub streamer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_started_at: Option<i64>,
    /// Keyed by stage name, in insertion order.
    pub stages: IndexMap<String, StageDiagnostic>,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for LivestreamDiagnosticsData {
    const NAME: &'static str = "livestream-diagnostics";
    type Key = String;
    fn key(&self) -> String {
        self.streamer_id.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LivestreamEventKind {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "metadata")]
    Metadata,
    #[serde(rename = "voice")]
    Voice,
    #[serde(rename = "summary")]
    Summary,
    #[serde(rename = "alert")]
    Alert,
    #[serde(rename = "feedback")]
    Feedback,
    #[serde(rename = "anomaly")]
    Anomaly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventStatus {
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "error")]
    Error,
}

/// `livestream-intelligence-event`, keyed by `eventId`. Field order follows
/// `{...input, eventId, createdAt}` as `recordLivestreamEvent` builds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamIntelligenceEventData {
    pub streamer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_started_at: Option<i64>,
    pub kind: LivestreamEventKind,
    pub status: EventStatus,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    pub event_id: String,
    pub created_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for LivestreamIntelligenceEventData {
    const NAME: &'static str = "livestream-intelligence-event";
    type Key = String;
    fn key(&self) -> String {
        self.event_id.clone()
    }
}

/// The `streamer-sessions` row (owned by `omni-live`), read for typical peaks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSessionsData {
    pub streamer_id: String,
    pub sessions: Vec<StreamSession>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for StreamSessionsData {
    const NAME: &'static str = "streamer-sessions";
    type Key = String;
    fn key(&self) -> String {
        self.streamer_id.clone()
    }
}

/// One completed live session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSession {
    pub started_at: f64,
    pub ended_at: f64,
    pub duration_ms: f64,
    pub peak_viewers: f64,
    pub title: String,
    pub platform: String,
    pub username: String,
    #[serde(flatten)]
    pub extra: Extra,
}
