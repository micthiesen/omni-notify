//! Persisted workspace rows. Field order
//! follows the object literals the TS engine writes; every row keeps unknown
//! fields in `extra` so read-modify-write never drops them.

use omni_api::workspaces::{
    WorkspaceAction, WorkspaceActionStatus, WorkspaceActionType, WorkspaceArtifactKind,
    WorkspaceArtifactRevision, WorkspaceEmailScope, WorkspaceMessage, WorkspaceMessageRole,
    WorkspacePapercut, WorkspacePapercutCategory, WorkspacePapercutStatus, WorkspaceSource,
    WorkspaceSourceKind, WorkspaceSubject, WorkspaceSubjectStatus,
};
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityDescriptor};
use serde::{Deserialize, Serialize};

/// `workspace-subject`, keyed by `workspaceId, subjectId`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectRow {
    pub workspace_id: String,
    pub subject_id: String,
    pub title: String,
    pub status: WorkspaceSubjectStatus,
    pub summary: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_researched_at: Option<i64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for SubjectRow {
    const NAME: &'static str = "workspace-subject";
    type Key = (String, String);
    fn key(&self) -> Self::Key {
        (self.workspace_id.clone(), self.subject_id.clone())
    }
}

impl SubjectRow {
    pub fn view(&self) -> WorkspaceSubject {
        WorkspaceSubject {
            workspace_id: self.workspace_id.clone(),
            subject_id: self.subject_id.clone(),
            title: self.title.clone(),
            status: self.status,
            summary: self.summary.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_researched_at: self.last_researched_at,
        }
    }
}

/// `workspace-artifact-revision`, keyed by `revisionId` (append-only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRevisionRow {
    pub workspace_id: String,
    pub subject_id: String,
    pub artifact_key: String,
    pub kind: WorkspaceArtifactKind,
    pub content: String,
    pub summary: String,
    pub revision_id: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for ArtifactRevisionRow {
    const NAME: &'static str = "workspace-artifact-revision";
    type Key = String;
    fn key(&self) -> Self::Key {
        self.revision_id.clone()
    }
}

impl ArtifactRevisionRow {
    pub fn view(&self) -> WorkspaceArtifactRevision {
        WorkspaceArtifactRevision {
            revision_id: self.revision_id.clone(),
            workspace_id: self.workspace_id.clone(),
            subject_id: self.subject_id.clone(),
            artifact_key: self.artifact_key.clone(),
            kind: self.kind,
            content: self.content.clone(),
            summary: self.summary.clone(),
            created_at: self.created_at,
            run_id: self.run_id.clone(),
        }
    }
}

/// `workspace-message`, keyed by `messageId`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    pub message_id: String,
    pub role: WorkspaceMessageRole,
    pub text: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for MessageRow {
    const NAME: &'static str = "workspace-message";
    type Key = String;
    fn key(&self) -> Self::Key {
        self.message_id.clone()
    }
}

impl MessageRow {
    pub fn view(&self) -> WorkspaceMessage {
        WorkspaceMessage {
            message_id: self.message_id.clone(),
            workspace_id: self.workspace_id.clone(),
            subject_id: self.subject_id.clone(),
            role: self.role,
            text: self.text.clone(),
            created_at: self.created_at,
            run_id: self.run_id.clone(),
        }
    }
}

/// `workspace-source`, keyed by `sourceId` (`email:<ws>:<subject>:<emailId>` for email).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    pub workspace_id: String,
    pub subject_id: String,
    pub source_id: String,
    pub kind: WorkspaceSourceKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_id: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Set only after the workspace run the email triggered completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggered_at: Option<i64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for SourceRow {
    const NAME: &'static str = "workspace-source";
    type Key = String;
    fn key(&self) -> Self::Key {
        self.source_id.clone()
    }
}

impl SourceRow {
    pub fn view(&self) -> WorkspaceSource {
        WorkspaceSource {
            source_id: self.source_id.clone(),
            workspace_id: self.workspace_id.clone(),
            subject_id: self.subject_id.clone(),
            kind: self.kind,
            title: self.title.clone(),
            url: self.url.clone(),
            excerpt: self.excerpt.clone(),
            email_id: self.email_id.clone(),
            created_at: self.created_at,
            run_id: self.run_id.clone(),
            triggered_at: self.triggered_at,
        }
    }
}

/// `workspace-action`, keyed by `actionId`. `payload` is the JSON string the
/// engine compares byte for byte when deduplicating pending proposals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRow {
    pub workspace_id: String,
    pub subject_id: String,
    pub action_id: String,
    #[serde(rename = "type")]
    pub action_type: WorkspaceActionType,
    pub status: WorkspaceActionStatus,
    pub title: String,
    pub description: String,
    pub payload: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for ActionRow {
    const NAME: &'static str = "workspace-action";
    type Key = String;
    fn key(&self) -> Self::Key {
        self.action_id.clone()
    }
}

impl ActionRow {
    pub fn view(&self) -> WorkspaceAction {
        WorkspaceAction {
            action_id: self.action_id.clone(),
            workspace_id: self.workspace_id.clone(),
            subject_id: self.subject_id.clone(),
            action_type: self.action_type,
            status: self.status,
            title: self.title.clone(),
            description: self.description.clone(),
            payload: self.payload.clone(),
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            result: self.result.clone(),
            run_id: self.run_id.clone(),
        }
    }
}

/// `workspace-email-scope`, keyed by `workspaceId, subjectId`: an approved
/// ingestion scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailScopeRow {
    pub workspace_id: String,
    pub subject_id: String,
    pub senders: Vec<String>,
    pub domains: Vec<String>,
    pub subject_keywords: Vec<String>,
    pub body_keywords: Vec<String>,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailScopeRow {
    const NAME: &'static str = "workspace-email-scope";
    type Key = (String, String);
    fn key(&self) -> Self::Key {
        (self.workspace_id.clone(), self.subject_id.clone())
    }
}

impl EmailScopeRow {
    pub fn scope(&self) -> WorkspaceEmailScope {
        WorkspaceEmailScope {
            senders: self.senders.clone(),
            domains: self.domains.clone(),
            subject_keywords: self.subject_keywords.clone(),
            body_keywords: self.body_keywords.clone(),
        }
    }
}

/// `workspace-papercut`, keyed by `papercutId`; deduplicated by `fingerprint`
/// while open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PapercutRow {
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
    pub papercut_id: String,
    pub fingerprint: String,
    pub occurrences: i64,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub status: WorkspacePapercutStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PapercutRow {
    const NAME: &'static str = "workspace-papercut";
    type Key = String;
    fn key(&self) -> Self::Key {
        self.papercut_id.clone()
    }
}

impl PapercutRow {
    pub fn view(&self) -> WorkspacePapercut {
        WorkspacePapercut {
            papercut_id: self.papercut_id.clone(),
            workspace_id: self.workspace_id.clone(),
            subject_id: self.subject_id.clone(),
            run_id: self.run_id.clone(),
            category: self.category,
            title: self.title.clone(),
            detail: self.detail.clone(),
            related_tool: self.related_tool.clone(),
            fingerprint: self.fingerprint.clone(),
            occurrences: self.occurrences,
            first_seen_at: self.first_seen_at,
            last_seen_at: self.last_seen_at,
            status: self.status,
            resolution: self.resolution.clone(),
        }
    }
}

/// `WorkspaceNotificationData["status"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationStatus {
    Pending,
    /// A provider attempt was reserved; found again on retry it becomes `unknown`.
    Sending,
    Sent,
    Unknown,
}

/// `workspace-notification`, keyed by `notificationId` (`action:<id>` or
/// `update:<runId>`): the durable Pushover outbox.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRow {
    pub notification_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    pub title: String,
    pub message: String,
    pub url: String,
    pub url_title: String,
    pub status: NotificationStatus,
    pub attempts: i64,
    pub created_at: i64,
    pub next_attempt_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for NotificationRow {
    const NAME: &'static str = "workspace-notification";
    type Key = String;
    fn key(&self) -> Self::Key {
        self.notification_id.clone()
    }
}

/// A notification before it is queued (`status`, `attempts`, `createdAt` and
/// `nextAttemptAt` are assigned when it enters the outbox).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationDraft {
    pub notification_id: String,
    pub workspace_id: String,
    pub subject_id: String,
    pub title: String,
    pub message: String,
    pub url: String,
    pub url_title: String,
}

impl NotificationDraft {
    pub fn queue(self, now: i64) -> NotificationRow {
        NotificationRow {
            notification_id: self.notification_id,
            workspace_id: self.workspace_id,
            subject_id: self.subject_id,
            title: self.title,
            message: self.message,
            url: self.url,
            url_title: self.url_title,
            status: NotificationStatus::Pending,
            attempts: 0,
            created_at: now,
            next_attempt_at: now,
            sent_at: None,
            last_error: None,
            extra: Extra::new(),
        }
    }
}

/// Every workspace entity, in data-manager order.
pub fn descriptors() -> Vec<EntityDescriptor> {
    vec![
        EntityDescriptor::of::<SubjectRow>(),
        EntityDescriptor::of::<ArtifactRevisionRow>(),
        EntityDescriptor::of::<MessageRow>(),
        EntityDescriptor::of::<SourceRow>(),
        EntityDescriptor::of::<ActionRow>(),
        EntityDescriptor::of::<EmailScopeRow>(),
        EntityDescriptor::of::<PapercutRow>(),
        EntityDescriptor::of::<NotificationRow>(),
    ]
}
