//! Typed API client functions (`frontend/src/api.ts`). DTOs come from
//! `omni-api`; [`dto`] holds the frontend copies of the WP14 shapes.

pub mod client;
pub mod dto;

use omni_api::briefings::BriefingsResponse;
use omni_api::claude::{
    ClaudeActivityResponse, ClaudeLinkFailure, ClaudeSessionsResponse, ClaudeTranscriptResponse,
};
use omni_api::common::encode_uri_component as enc;
use omni_api::costs::{CostRange, CostsResponse};
use omni_api::email::{
    EmailActivitiesResponse, EmailActivityLogsResponse, EmailActivityResponse,
    EmailFeedbackListResponse, EmailFeedbackResponse, EmailFeedbackVerdict, EmailPipelineName,
    EmailRuleInput, EmailRuleUpsertResponse, EmailRulesResponse,
};
use omni_api::intelligence::{FeedbackResponse, FeedbackVerdict, IntelligenceDetailsResponse};
use omni_api::mcp_activity::{McpActivityResponse, McpCallStatus};
use omni_api::media::{
    RecommendationFeedback, RecommendationResponse, RecommendationsResponse, TasteProfileResponse,
};
use omni_api::podcasts::{
    PodcastFeedback, PodcastRecommendationResponse, PodcastRecommendationsResponse,
    PodcastTasteProfileResponse,
};
use omni_api::presspods::{PressPodsEpisodeResponse, PressPodsJobResponse, PressPodsListResponse};
use omni_api::runs::{RunLogsResponse, RunsResponse};
use omni_api::streamers::{StreamSessionsResponse, StreamerMetricsResponse};
use omni_api::tasks::TasksResponse;
use omni_api::workspaces::{
    WorkspaceActionResponse, WorkspaceResponse, WorkspaceSubjectResponse, WorkspaceSubjectStatus,
    WorkspaceSubjectUpdated, WorkspacesResponse,
};
use serde_json::{Map, Value, json};

pub use client::{ApiClientError, NO_BODY, delete, get, post};
pub use dto::{
    DataEntitiesResponse, DataEntity, DataRow, DataRowsResponse, DataStorageSummary, Deleted,
    RunId, Snapshot,
};

/// Builds `?a=1&b=2` (`URLSearchParams` order and encoding of plain values).
fn query(params: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = params
        .iter()
        .filter_map(|(key, value)| value.as_ref().map(|v| format!("{key}={}", form_encode(v))))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// `application/x-www-form-urlencoded` serialization, as `URLSearchParams` does.
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

// ----- tasks, runs, snapshot, data -----

pub async fn fetch_tasks() -> Result<TasksResponse, ApiClientError> {
    get("/api/tasks").await
}

pub async fn fetch_snapshot() -> Result<Snapshot, ApiClientError> {
    get("/api/snapshot").await
}

pub async fn fetch_data_entities() -> Result<DataEntitiesResponse, ApiClientError> {
    get("/api/data/entities").await
}

pub async fn fetch_data_rows(slug: &str) -> Result<DataRowsResponse, ApiClientError> {
    get(&format!("/api/data/entities/{}", enc(slug))).await
}

pub async fn delete_data_row(slug: &str, key: &DataRow) -> Result<Deleted, ApiClientError> {
    delete(
        &format!("/api/data/entities/{}", enc(slug)),
        Some(&json!({ "key": key })),
    )
    .await
}

pub async fn fetch_task_runs(
    task: Option<&str>,
    limit: Option<u32>,
) -> Result<RunsResponse, ApiClientError> {
    let q = query(&[
        ("task", task.filter(|t| !t.is_empty()).map(str::to_owned)),
        ("limit", limit.map(|l| l.to_string())),
    ]);
    get(&format!("/api/task-runs{q}")).await
}

pub async fn fetch_run_logs(run_id: &str) -> Result<RunLogsResponse, ApiClientError> {
    get(&format!("/api/task-runs/{}/logs", enc(run_id))).await
}

pub fn run_log_stream_url(run_id: &str) -> String {
    format!("/api/task-runs/{}/logs/stream", enc(run_id))
}

/// `POST /api/tasks/:name/run`, or the recommendation run routes when a
/// `max_recommendations` override is given.
pub async fn run_task_request(
    name: &str,
    max_recommendations: Option<u32>,
) -> Result<RunId, ApiClientError> {
    if let Some(max) = max_recommendations {
        let path = if name == "PodcastRecs" {
            "/api/podcast-recommendations/run"
        } else {
            "/api/recommendations/run"
        };
        return post(path, Some(&json!({ "maxRecommendations": max }))).await;
    }
    post(&format!("/api/tasks/{}/run", enc(name)), NO_BODY).await
}

// ----- streamers -----

pub async fn fetch_streamer_metrics(id: &str) -> Result<StreamerMetricsResponse, ApiClientError> {
    get(&format!("/api/streamers/{}/metrics", enc(id))).await
}

pub async fn fetch_streamer_sessions(id: &str) -> Result<StreamSessionsResponse, ApiClientError> {
    get(&format!("/api/streamers/{}/sessions", enc(id))).await
}

pub async fn fetch_livestream_intelligence_details(
    id: &str,
    limit: u32,
) -> Result<IntelligenceDetailsResponse, ApiClientError> {
    get(&format!(
        "/api/streamers/{}/intelligence-details?limit={limit}",
        enc(id)
    ))
    .await
}

pub async fn submit_livestream_feedback(
    streamer_id: &str,
    alert_id: &str,
    verdict: FeedbackVerdict,
    note: Option<&str>,
) -> Result<FeedbackResponse, ApiClientError> {
    let mut body = Map::new();
    body.insert("alertId".into(), json!(alert_id));
    body.insert(
        "verdict".into(),
        serde_json::to_value(verdict).unwrap_or(Value::Null),
    );
    if let Some(note) = note {
        body.insert("note".into(), json!(note));
    }
    post(
        &format!("/api/streamers/{}/intelligence-feedback", enc(streamer_id)),
        Some(&Value::Object(body)),
    )
    .await
}

// ----- media recommendations -----

pub async fn fetch_recommendations() -> Result<RecommendationsResponse, ApiClientError> {
    get("/api/recommendations").await
}

pub async fn fetch_taste_profile() -> Result<TasteProfileResponse, ApiClientError> {
    get("/api/recommendations/taste-profile").await
}

pub async fn fetch_recommendation(id: &str) -> Result<RecommendationResponse, ApiClientError> {
    get(&format!("/api/recommendations/{}", enc(id))).await
}

fn feedback_body<F: serde::Serialize>(feedback: Option<F>, note: Option<&str>) -> Value {
    let mut body = Map::new();
    if let Some(feedback) = feedback {
        body.insert(
            "feedback".into(),
            serde_json::to_value(feedback).unwrap_or(Value::Null),
        );
    }
    if let Some(note) = note {
        body.insert("note".into(), json!(note));
    }
    Value::Object(body)
}

pub async fn send_recommendation_feedback(
    id: &str,
    feedback: Option<RecommendationFeedback>,
    note: Option<&str>,
) -> Result<RecommendationResponse, ApiClientError> {
    post(
        &format!("/api/recommendations/{}/feedback", enc(id)),
        Some(&feedback_body(feedback, note)),
    )
    .await
}

// ----- podcasts -----

pub async fn fetch_podcast_recommendations()
-> Result<PodcastRecommendationsResponse, ApiClientError> {
    get("/api/podcast-recommendations").await
}

pub async fn fetch_podcast_taste_profile() -> Result<PodcastTasteProfileResponse, ApiClientError> {
    get("/api/podcast-recommendations/taste-profile").await
}

pub async fn fetch_podcast_recommendation(
    id: &str,
) -> Result<PodcastRecommendationResponse, ApiClientError> {
    get(&format!("/api/podcast-recommendations/{}", enc(id))).await
}

pub async fn send_podcast_recommendation_feedback(
    id: &str,
    feedback: Option<PodcastFeedback>,
    note: Option<&str>,
) -> Result<PodcastRecommendationResponse, ApiClientError> {
    post(
        &format!("/api/podcast-recommendations/{}/feedback", enc(id)),
        Some(&feedback_body(feedback, note)),
    )
    .await
}

// ----- PressPods -----

pub async fn fetch_press_pods() -> Result<PressPodsListResponse, ApiClientError> {
    get("/api/press-pods/episodes").await
}

pub async fn fetch_press_pods_episode(
    episode_id: &str,
) -> Result<PressPodsEpisodeResponse, ApiClientError> {
    get(&format!("/api/press-pods/episodes/{}", enc(episode_id))).await
}

pub async fn submit_press_pods_url(url: &str) -> Result<PressPodsJobResponse, ApiClientError> {
    post("/api/press-pods/submit", Some(&json!({ "url": url }))).await
}

pub async fn retry_press_pods_job(job_id: &str) -> Result<PressPodsJobResponse, ApiClientError> {
    post(
        &format!("/api/press-pods/jobs/{}/retry", enc(job_id)),
        NO_BODY,
    )
    .await
}

pub async fn dismiss_press_pods_job(job_id: &str) -> Result<Deleted, ApiClientError> {
    delete(&format!("/api/press-pods/jobs/{}", enc(job_id)), NO_BODY).await
}

pub async fn delete_press_pods_episode(episode_id: &str) -> Result<Deleted, ApiClientError> {
    delete(
        &format!("/api/press-pods/episodes/{}", enc(episode_id)),
        NO_BODY,
    )
    .await
}

pub async fn retry_press_pods_episode(
    episode_id: &str,
) -> Result<PressPodsJobResponse, ApiClientError> {
    post(
        &format!("/api/press-pods/episodes/{}/retry", enc(episode_id)),
        NO_BODY,
    )
    .await
}

// ----- briefings and workspaces -----

pub async fn fetch_briefings() -> Result<BriefingsResponse, ApiClientError> {
    get("/api/briefings").await
}

pub async fn fetch_workspaces() -> Result<WorkspacesResponse, ApiClientError> {
    get("/api/workspaces").await
}

pub async fn fetch_workspace(workspace_id: &str) -> Result<WorkspaceResponse, ApiClientError> {
    get(&format!("/api/workspaces/{}", enc(workspace_id))).await
}

pub async fn fetch_workspace_subject(
    workspace_id: &str,
    subject_id: &str,
) -> Result<WorkspaceSubjectResponse, ApiClientError> {
    get(&format!(
        "/api/workspaces/{}/subjects/{}",
        enc(workspace_id),
        enc(subject_id)
    ))
    .await
}

pub async fn send_workspace_message(
    workspace_id: &str,
    message: &str,
    subject_id: Option<&str>,
) -> Result<RunId, ApiClientError> {
    let mut body = Map::new();
    body.insert("message".into(), json!(message));
    if let Some(subject_id) = subject_id {
        body.insert("subjectId".into(), json!(subject_id));
    }
    post(
        &format!("/api/workspaces/{}/messages", enc(workspace_id)),
        Some(&Value::Object(body)),
    )
    .await
}

pub async fn set_workspace_subject_status(
    workspace_id: &str,
    subject_id: &str,
    status: WorkspaceSubjectStatus,
) -> Result<WorkspaceSubjectUpdated, ApiClientError> {
    post(
        &format!(
            "/api/workspaces/{}/subjects/{}/status",
            enc(workspace_id),
            enc(subject_id)
        ),
        Some(&json!({ "status": status })),
    )
    .await
}

/// `"approve"` or `"reject"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionResolution {
    Approve,
    Reject,
}

pub async fn resolve_workspace_action(
    action_id: &str,
    resolution: ActionResolution,
) -> Result<WorkspaceActionResponse, ApiClientError> {
    let verb = match resolution {
        ActionResolution::Approve => "approve",
        ActionResolution::Reject => "reject",
    };
    post(
        &format!("/api/workspace-actions/{}/{verb}", enc(action_id)),
        NO_BODY,
    )
    .await
}

// ----- costs -----

/// `?days=` value of a range.
pub fn cost_range_param(range: CostRange) -> String {
    match range {
        CostRange::Days(days) => days.to_string(),
        CostRange::All => "all".to_owned(),
    }
}

pub async fn fetch_costs(range: CostRange) -> Result<CostsResponse, ApiClientError> {
    get(&format!("/api/costs?days={}", cost_range_param(range))).await
}

// ----- email -----

pub async fn fetch_email_activity(
    pipeline: Option<EmailPipelineName>,
    limit: Option<u32>,
) -> Result<EmailActivitiesResponse, ApiClientError> {
    let q = query(&[
        ("pipeline", pipeline.map(|p| p.as_str().to_owned())),
        ("limit", limit.map(|l| l.to_string())),
    ]);
    get(&format!("/api/email-activity{q}")).await
}

pub async fn fetch_email_activity_logs(
    activity_id: &str,
) -> Result<EmailActivityLogsResponse, ApiClientError> {
    get(&format!("/api/email-activity/{}/logs", enc(activity_id))).await
}

pub async fn fetch_email_rules() -> Result<EmailRulesResponse, ApiClientError> {
    get("/api/email-rules").await
}

pub async fn create_email_rule(
    input: &EmailRuleInput,
) -> Result<EmailRuleUpsertResponse, ApiClientError> {
    post("/api/email-rules", Some(input)).await
}

pub async fn delete_email_rule(rule_id: &str) -> Result<Deleted, ApiClientError> {
    delete(&format!("/api/email-rules/{}", enc(rule_id)), NO_BODY).await
}

pub async fn fetch_email_feedback() -> Result<EmailFeedbackListResponse, ApiClientError> {
    get("/api/email-feedback").await
}

pub async fn send_email_activity_feedback(
    activity_id: &str,
    verdict: Option<EmailFeedbackVerdict>,
    note: Option<&str>,
) -> Result<EmailFeedbackResponse, ApiClientError> {
    let mut body = Map::new();
    body.insert(
        "verdict".into(),
        serde_json::to_value(verdict).unwrap_or(Value::Null),
    );
    if let Some(note) = note {
        body.insert("note".into(), json!(note));
    }
    post(
        &format!("/api/email-activity/{}/feedback", enc(activity_id)),
        Some(&Value::Object(body)),
    )
    .await
}

pub async fn reprocess_email_activity(
    activity_id: &str,
) -> Result<EmailActivityResponse, ApiClientError> {
    post(
        &format!("/api/email-activity/{}/reprocess", enc(activity_id)),
        NO_BODY,
    )
    .await
}

pub async fn forget_parcel_delivery(tracking_number: &str) -> Result<Deleted, ApiClientError> {
    delete(
        &format!("/api/parcel-tracker/deliveries/{}", enc(tracking_number)),
        NO_BODY,
    )
    .await
}

// ----- MCP and Claude Code activity -----

pub async fn fetch_mcp_activity(
    limit: Option<u32>,
    tool: Option<&str>,
    status: Option<McpCallStatus>,
    before: Option<i64>,
) -> Result<McpActivityResponse, ApiClientError> {
    let status = status.map(|s| match s {
        McpCallStatus::Running => "running",
        McpCallStatus::Ok => "ok",
        McpCallStatus::Error => "error",
        McpCallStatus::Interrupted => "interrupted",
    });
    let q = query(&[
        ("limit", limit.map(|l| l.to_string())),
        ("tool", tool.filter(|t| !t.is_empty()).map(str::to_owned)),
        ("status", status.map(str::to_owned)),
        ("before", before.map(|b| b.to_string())),
    ]);
    get(&format!("/api/mcp/activity{q}")).await
}

pub async fn fetch_claude_activity(limit: u32) -> Result<ClaudeActivityResponse, ApiClientError> {
    get(&format!("/api/claude/activity?limit={limit}")).await
}

/// The host link answered 503 (not configured, offline, disabled) or reported
/// a failure (502); carries the server's message and code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeLinkError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

/// `ApiClientError | ClaudeLinkError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeLinkGetError {
    Link(ClaudeLinkError),
    Client(ApiClientError),
}

impl ClaudeLinkGetError {
    pub fn message(&self) -> &str {
        match self {
            ClaudeLinkGetError::Link(link) => &link.message,
            ClaudeLinkGetError::Client(client) => client.message(),
        }
    }
}

fn read_link_error(response: &client::RawResponse) -> ClaudeLinkError {
    #[derive(serde::Deserialize)]
    struct Body {
        error: String,
        #[serde(default)]
        code: Option<String>,
    }
    match serde_json::from_str::<Body>(&response.body) {
        Ok(body) => ClaudeLinkError {
            status: response.status,
            code: body.code.unwrap_or_else(|| "unavailable".to_owned()),
            message: body.error,
        },
        Err(_) => ClaudeLinkError {
            status: response.status,
            code: "unavailable".to_owned(),
            message: format!("HTTP {}: {}", response.status, response.status_text),
        },
    }
}

/// Live host queries skip the GET restart retry: a 503 usually means the link
/// is offline or disabled, and the page polls again on its own schedule.
async fn claude_link_get<T: serde::de::DeserializeOwned>(
    path: &str,
) -> Result<T, ClaudeLinkGetError> {
    let response = client::send(client::Method::Get, path, None)
        .await
        .map_err(ClaudeLinkGetError::Client)?;
    if response.status == 502 || response.status == 503 {
        return Err(ClaudeLinkGetError::Link(read_link_error(&response)));
    }
    client::decode_response(path, &response).map_err(ClaudeLinkGetError::Client)
}

/// Kept so a [`ClaudeLinkFailure`] body is the documented error shape.
pub type ClaudeLinkFailureBody = ClaudeLinkFailure;

pub async fn fetch_claude_sessions(
    include_stopped: bool,
    limit: u32,
) -> Result<ClaudeSessionsResponse, ClaudeLinkGetError> {
    claude_link_get(&format!(
        "/api/claude/sessions?includeStopped={include_stopped}&limit={limit}"
    ))
    .await
}

pub async fn fetch_claude_transcript(
    session_id: &str,
    limit: u32,
    cursor: Option<i64>,
) -> Result<ClaudeTranscriptResponse, ClaudeLinkGetError> {
    let cursor = cursor.map(|c| format!("&cursor={c}")).unwrap_or_default();
    claude_link_get(&format!(
        "/api/claude/sessions/{}/transcript?limit={limit}{cursor}",
        enc(session_id)
    ))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_match_url_search_params() {
        assert_eq!(
            query(&[("task", Some("Live Check".into())), ("limit", None)]),
            "?task=Live+Check"
        );
        assert_eq!(query(&[("task", None)]), "");
        assert_eq!(form_encode("a/b:c"), "a%2Fb%3Ac");
        assert_eq!(
            run_log_stream_url("LiveCheckTask:1"),
            "/api/task-runs/LiveCheckTask%3A1/logs/stream"
        );
        assert_eq!(cost_range_param(CostRange::All), "all");
    }
}
