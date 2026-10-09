//! Workspace definitions, dossiers, actions and papercuts, for the
//! `/api/workspaces*`, `/api/workspace-actions/*` and `/api/workspace-papercuts*`
//! routes.
//!
//! Timestamps are epoch milliseconds. Optional fields are omitted when absent;
//! nullable fields are always present.

use serde::{Deserialize, Serialize};

use crate::common::Ms;

/// `WorkspaceSubjectStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceSubjectStatus {
    Active,
    Paused,
    Completed,
    Archived,
}

impl WorkspaceSubjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Archived => "archived",
        }
    }
}

/// `WorkspaceArtifactKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceArtifactKind {
    Markdown,
    Structured,
    EvidenceLedger,
    Timeline,
    Collection,
}

/// `WorkspaceArtifactDefinition`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceArtifactDefinition {
    pub key: String,
    pub title: String,
    pub kind: WorkspaceArtifactKind,
    pub instructions: String,
}

/// `WorkspaceDefinition`, including the agent instructions (the REST API
/// serves the full definition; the MCP tools exclude the prompts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDefinition {
    pub id: String,
    pub title: String,
    pub description: String,
    pub subject_label: String,
    pub subject_label_plural: String,
    pub task_name: String,
    pub schedule: String,
    /// `false` for workspaces that only progress in response to user input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_runs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_placeholder: Option<String>,
    pub instructions: String,
    pub artifacts: Vec<WorkspaceArtifactDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSubject {
    pub workspace_id: String,
    pub subject_id: String,
    pub title: String,
    pub status: WorkspaceSubjectStatus,
    pub summary: String,
    pub created_at: Ms,
    pub updated_at: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_researched_at: Option<Ms>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceArtifactRevision {
    pub revision_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    pub artifact_key: String,
    pub kind: WorkspaceArtifactKind,
    pub content: String,
    pub summary: String,
    pub created_at: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// `WorkspaceMessageData["role"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMessageRole {
    User,
    Assistant,
    System,
}

impl WorkspaceMessageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMessage {
    pub message_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    pub role: WorkspaceMessageRole,
    pub text: String,
    pub created_at: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// `WorkspaceSourceData["kind"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceSourceKind {
    Web,
    Email,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSource {
    pub source_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    pub kind: WorkspaceSourceKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_id: Option<String>,
    pub created_at: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggered_at: Option<Ms>,
}

/// `WorkspaceActionType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceActionType {
    EmailScope,
    CalendarEvent,
}

/// `WorkspaceActionStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceActionStatus {
    Pending,
    Approved,
    Rejected,
    Failed,
}

impl WorkspaceActionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }
}

/// A workspace action; `payload` is the JSON string compared byte for byte
/// when deduplicating proposals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAction {
    pub action_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    #[serde(rename = "type")]
    pub action_type: WorkspaceActionType,
    pub status: WorkspaceActionStatus,
    pub title: String,
    pub description: String,
    pub payload: String,
    pub created_at: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// A workspace email scope (also the subject route's `emailScope`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEmailScope {
    pub senders: Vec<String>,
    pub domains: Vec<String>,
    pub subject_keywords: Vec<String>,
    pub body_keywords: Vec<String>,
}

/// `WorkspacePapercutCategory`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspacePapercutCategory {
    MissingCapability,
    PoorSourceData,
    IntegrationFriction,
    WorkflowGap,
    PromptProblem,
    UiGap,
}

impl WorkspacePapercutCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingCapability => "missing-capability",
            Self::PoorSourceData => "poor-source-data",
            Self::IntegrationFriction => "integration-friction",
            Self::WorkflowGap => "workflow-gap",
            Self::PromptProblem => "prompt-problem",
            Self::UiGap => "ui-gap",
        }
    }
}

/// `WorkspacePapercutData["status"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspacePapercutStatus {
    Open,
    Addressed,
    Dismissed,
}

/// A workspace papercut as the REST API serves it (fingerprint included).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePapercut {
    pub papercut_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub category: WorkspacePapercutCategory,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_tool: Option<String>,
    pub fingerprint: String,
    pub occurrences: i64,
    pub first_seen_at: Ms,
    pub last_seen_at: Ms,
    pub status: WorkspacePapercutStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

/// One entry of `GET /api/workspaces`: the definition plus counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOverview {
    #[serde(flatten)]
    pub definition: WorkspaceDefinition,
    pub subjects: Vec<WorkspaceSubject>,
    pub active_subject_count: u64,
    pub pending_action_count: u64,
    pub open_papercut_count: u64,
}

/// `GET /api/workspaces`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacesResponse {
    pub workspaces: Vec<WorkspaceOverview>,
}

/// `GET /api/workspaces/:workspaceId` (papercuts are the open ones).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceResponse {
    pub workspace: WorkspaceDefinition,
    pub subjects: Vec<WorkspaceSubject>,
    pub actions: Vec<WorkspaceAction>,
    pub papercuts: Vec<WorkspacePapercut>,
}

/// `GET /api/workspaces/:workspaceId/subjects/:subjectId`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSubjectResponse {
    pub workspace: WorkspaceDefinition,
    pub subject: WorkspaceSubject,
    /// Latest revision per artifact key, sorted by key.
    pub artifacts: Vec<WorkspaceArtifactRevision>,
    /// Every revision, newest first.
    pub artifact_revisions: Vec<WorkspaceArtifactRevision>,
    /// The 100 most recent, oldest first.
    pub messages: Vec<WorkspaceMessage>,
    /// The 100 most recent, newest first.
    pub sources: Vec<WorkspaceSource>,
    pub actions: Vec<WorkspaceAction>,
    pub email_scope: Option<WorkspaceEmailScope>,
    /// Open papercuts of the workspace that are unscoped or scoped to this subject.
    pub papercuts: Vec<WorkspacePapercut>,
}

/// `POST /api/workspaces/:workspaceId/messages` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMessageRequest {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
}

/// `202` reply to a workspace message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMessageAccepted {
    pub run_id: String,
}

/// `POST /api/workspaces/:workspaceId/subjects/:subjectId/status` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSubjectStatusRequest {
    pub status: WorkspaceSubjectStatus,
}

/// Reply to a subject status change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSubjectUpdated {
    pub subject: WorkspaceSubject,
}

/// Reply to an action approval or rejection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceActionResponse {
    pub action: WorkspaceAction,
}

/// `GET /api/workspace-papercuts?workspaceId=&status=`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePapercutsResponse {
    pub papercuts: Vec<WorkspacePapercut>,
}

/// Resolution status accepted by the papercut resolve route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspacePapercutResolution {
    Addressed,
    Dismissed,
}

impl From<WorkspacePapercutResolution> for WorkspacePapercutStatus {
    fn from(value: WorkspacePapercutResolution) -> Self {
        match value {
            WorkspacePapercutResolution::Addressed => Self::Addressed,
            WorkspacePapercutResolution::Dismissed => Self::Dismissed,
        }
    }
}

/// `POST /api/workspace-papercuts/:papercutId/resolve` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePapercutResolveRequest {
    pub status: WorkspacePapercutResolution,
    pub resolution: String,
}

/// Reply to a papercut resolution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePapercutResponse {
    pub papercut: WorkspacePapercut,
}

/// Path builders for the workspace routes (the frontend uses these).
pub mod paths {
    use crate::common::encode_uri_component;

    pub const WORKSPACES: &str = "/api/workspaces";
    pub const PAPERCUTS: &str = "/api/workspace-papercuts";

    /// `GET /api/workspaces/:workspaceId`.
    pub fn workspace(workspace_id: &str) -> String {
        format!("{WORKSPACES}/{}", encode_uri_component(workspace_id))
    }

    /// `GET /api/workspaces/:workspaceId/subjects/:subjectId`.
    pub fn subject(workspace_id: &str, subject_id: &str) -> String {
        format!(
            "{}/subjects/{}",
            workspace(workspace_id),
            encode_uri_component(subject_id)
        )
    }

    /// `POST /api/workspaces/:workspaceId/messages`.
    pub fn messages(workspace_id: &str) -> String {
        format!("{}/messages", workspace(workspace_id))
    }

    /// `POST /api/workspaces/:workspaceId/subjects/:subjectId/status`.
    pub fn subject_status(workspace_id: &str, subject_id: &str) -> String {
        format!("{}/status", subject(workspace_id, subject_id))
    }

    /// `POST /api/workspace-actions/:actionId/approve`.
    pub fn approve_action(action_id: &str) -> String {
        format!(
            "/api/workspace-actions/{}/approve",
            encode_uri_component(action_id)
        )
    }

    /// `POST /api/workspace-actions/:actionId/reject`.
    pub fn reject_action(action_id: &str) -> String {
        format!(
            "/api/workspace-actions/{}/reject",
            encode_uri_component(action_id)
        )
    }

    /// `GET /api/workspace-papercuts?workspaceId=&status=`.
    pub fn papercuts(workspace_id: Option<&str>, status: Option<&str>) -> String {
        let mut query = Vec::new();
        if let Some(workspace_id) = workspace_id {
            query.push(format!(
                "workspaceId={}",
                encode_uri_component(workspace_id)
            ));
        }
        if let Some(status) = status {
            query.push(format!("status={}", encode_uri_component(status)));
        }
        if query.is_empty() {
            PAPERCUTS.to_owned()
        } else {
            format!("{PAPERCUTS}?{}", query.join("&"))
        }
    }

    /// `POST /api/workspace-papercuts/:papercutId/resolve`.
    pub fn resolve_papercut(papercut_id: &str) -> String {
        format!("{PAPERCUTS}/{}/resolve", encode_uri_component(papercut_id))
    }
}
