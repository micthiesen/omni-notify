//! Owned by WP05: livestream intelligence details and feedback
//! (`GET /api/streamers/:id/intelligence-details`,
//! `POST /api/streamers/:id/intelligence-feedback`).
//!
//! These mirror the persisted documents (`src/live-check/intelligence/types.ts`).
//! Metric values are `number | string | boolean | null`, carried as JSON values.
//! TS `field?: T | null` reads as `Option<T>` (absent and null are both `None`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::common::Ms;

pub type MetricMap = Map<String, Value>;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeedbackVerdict {
    #[serde(rename = "useful")]
    Useful,
    #[serde(rename = "not_useful")]
    NotUseful,
    #[serde(rename = "false_positive")]
    FalsePositive,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticMetadata {
    pub headline: String,
    pub topics: Vec<String>,
    pub content_kind: String,
    pub importance: f64,
    pub reason: String,
    pub updated_at: Ms,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerTrend {
    pub percent_change: f64,
    pub viewers_per_minute: f64,
    pub dgg_percent_change: Option<f64>,
    pub anomalous: bool,
    pub reason: Option<String>,
    #[serde(default)]
    pub current_viewers: Option<f64>,
    #[serde(default)]
    pub baseline_viewers: Option<f64>,
    #[serde(default)]
    pub current_dgg_viewers: Option<f64>,
    #[serde(default)]
    pub baseline_dgg_viewers: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_samples: Option<f64>,
    #[serde(default)]
    pub typical_peak_viewers: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_observations: Option<f64>,
    #[serde(default)]
    pub suppression_reason: Option<String>,
    pub updated_at: Ms,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamChapter {
    pub chapter_id: String,
    pub started_at: f64,
    pub title: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollingSummary {
    pub text: String,
    pub topic: String,
    pub confidence: f64,
    pub transcript_excerpt: String,
    pub updated_at: Ms,
    pub window_seconds: f64,
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
    pub detected_at: Ms,
    pub reason: String,
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
    pub created_at: Ms,
}

/// The `livestream-intelligence` document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamIntelligence {
    pub streamer_id: String,
    pub session_started_at: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<SemanticMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend: Option<ViewerTrend>,
    pub relevance_score: f64,
    pub relevance_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RollingSummary>,
    pub chapters: Vec<LivestreamChapter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destiny_presence: Option<DestinyPresence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_alert: Option<LivestreamAlertRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerted_at_by_type: Option<Map<String, Value>>,
    pub updated_at: Ms,
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
    pub started_at: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<MetricMap>,
}

/// The `livestream-diagnostics` document; `stages` is keyed by
/// `metadata | voice | summary | alert`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamDiagnostics {
    pub streamer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_started_at: Option<Ms>,
    pub stages: Map<String, Value>,
    pub updated_at: Ms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
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

/// One `livestream-intelligence-event` timeline entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamEvent {
    pub event_id: String,
    pub streamer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_started_at: Option<Ms>,
    pub created_at: Ms,
    pub kind: EventKind,
    pub status: EventStatus,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<MetricMap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueDiagnostics {
    pub running: u64,
    pub queued: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeQueues {
    pub capture: QueueDiagnostics,
    pub speech: QueueDiagnostics,
    pub llm: QueueDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBudget {
    pub spent_cents: f64,
    pub limit_cents: f64,
    pub remaining_cents: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIntervals {
    pub voice_seconds: u64,
    pub summary_seconds: u64,
}

/// `LivestreamRuntimeDiagnostics` (only while the service is enabled).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostics {
    pub enabled: bool,
    pub voiceprint_loaded: bool,
    pub model: String,
    pub queues: RuntimeQueues,
    pub active_stream_count: u64,
    pub active_voice_target_count: u64,
    pub budget: RuntimeBudget,
    pub intervals: RuntimeIntervals,
}

/// `GET /api/streamers/:id/intelligence-details?limit=N` (default 100, max 200).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntelligenceDetailsResponse {
    pub intelligence: Option<LivestreamIntelligence>,
    pub diagnostics: Option<LivestreamDiagnostics>,
    pub events: Vec<LivestreamEvent>,
    pub runtime: Option<RuntimeDiagnostics>,
    pub generated_at: Ms,
}

/// `POST /api/streamers/:id/intelligence-feedback` body. `alertId` is a UUID and
/// `note` at most 500 characters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRequest {
    pub alert_id: String,
    pub verdict: FeedbackVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One `livestream-feedback` document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamFeedback {
    pub feedback_id: String,
    pub streamer_id: String,
    pub alert_id: String,
    pub alert_type: LivestreamAlertType,
    pub verdict: FeedbackVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: Ms,
}

/// `201 {"feedback": ...}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeedbackResponse {
    pub feedback: LivestreamFeedback,
}
