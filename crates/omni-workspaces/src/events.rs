//! `workspace.updated` publications, made after a run's output commits.
//!
//! A pending action's dedup key is its action ID; a reply's is the run ID
//! (or, for a run without one, its first assistant message), so a reprocessed
//! run does not fire again. A proposal identical to a pending action is not
//! created, so it publishes nothing.

use omni_api::events::{WORKSPACE_UPDATED, WorkspaceUpdateKind, WorkspaceUpdated, bounded_title};
use omni_runtime::ports::{EventPublication, Ports};
use serde_json::Value;

use crate::entities::ActionRow;

const LOG: &str = "Workspaces";

/// The assistant reply a committed run produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedReply {
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub message_id: String,
    pub run_id: Option<String>,
    pub created_at: i64,
}

fn publication(
    dedup_key: String,
    occurred_at_ms: i64,
    payload: WorkspaceUpdated,
) -> Option<EventPublication> {
    let Ok(Value::Object(data)) = serde_json::to_value(payload) else {
        return None;
    };
    Some(EventPublication {
        name: WORKSPACE_UPDATED,
        dedup_key,
        occurred_at_ms,
        data,
    })
}

/// `action_pending` for a newly created action.
pub fn action_pending(action: &ActionRow) -> Option<EventPublication> {
    let action_type = serde_json::to_value(action.action_type)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned));
    publication(
        format!("{}:pending", action.action_id),
        action.created_at,
        WorkspaceUpdated {
            workspace_id: action.workspace_id.clone(),
            subject_id: Some(action.subject_id.clone()),
            kind: WorkspaceUpdateKind::ActionPending,
            action_id: Some(action.action_id.clone()),
            action_type,
            title: bounded_title(&action.title),
            run_id: action.run_id.clone(),
        },
    )
}

/// `reply_ready` for a run's assistant reply.
pub fn reply_ready(reply: &CommittedReply) -> Option<EventPublication> {
    let key = match &reply.run_id {
        Some(run_id) => format!("run:{run_id}:reply"),
        None => format!("message:{}:reply", reply.message_id),
    };
    publication(
        key,
        reply.created_at,
        WorkspaceUpdated {
            workspace_id: reply.workspace_id.clone(),
            subject_id: reply.subject_id.clone(),
            kind: WorkspaceUpdateKind::ReplyReady,
            action_id: None,
            action_type: None,
            title: None,
            run_id: reply.run_id.clone(),
        },
    )
}

/// Publishes when MCP Events are enabled. A failure is logged: the run's
/// output is already committed and its notifications still deliver.
pub async fn publish_all(ports: &Ports, events: Vec<EventPublication>) {
    if ports.event_publisher().is_none() {
        return;
    }
    for event in events {
        if let Err(error) = ports.publish_event(&event).await {
            tracing::warn!(target: LOG, error = %error, "Workspace event not published");
        }
    }
}
