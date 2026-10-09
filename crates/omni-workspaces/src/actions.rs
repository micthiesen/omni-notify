//! Approving and rejecting workspace proposals (`src/workspaces/actions.ts`).
//!
//! Nothing a workspace agent proposes takes effect until the user approves it
//! here (REST route or Executor-approved MCP tool). Approval and rejection of
//! one action are mutually exclusive in process, an approval runs to
//! completion even if its caller goes away, and a calendar approval uses the
//! deterministic UID `workspace-<actionId>@omni-notify`, so a retry after an
//! uncertain failure finds the existing event (412) instead of duplicating it.

use omni_api::workspaces::{WorkspaceActionStatus, WorkspaceActionType, WorkspaceEmailScope};
use omni_runtime::ports::{CalendarCreateOutcome, CalendarEventInput, PortError};
use serde::Deserialize;

use crate::entities::ActionRow;
use crate::error::{WorkspaceError, op};
use crate::service::WorkspaceService;
use crate::text::{js_len, js_trim};

const LOG: &str = "Workspaces";

/// The deterministic CalDAV UID of an approved calendar proposal.
pub fn calendar_uid(action_id: &str) -> String {
    format!("workspace-{action_id}@omni-notify")
}

/// Releases an action's in-process claim on every exit path.
struct Resolving {
    service: WorkspaceService,
    action_id: String,
}

impl Drop for Resolving {
    fn drop(&mut self) {
        self.service
            .inner
            .resolving
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.action_id);
    }
}

fn decode_payload<T: for<'de> Deserialize<'de>>(
    action_id: &str,
    payload: &str,
) -> Result<T, WorkspaceError> {
    let value: serde_json::Value = serde_json::from_str(payload).map_err(|e| {
        WorkspaceError::validation_with(format!("Workspace action {action_id} has invalid JSON"), e)
    })?;
    serde_json::from_value(value).map_err(|e| {
        WorkspaceError::validation_with(
            format!("Workspace action {action_id} has an invalid payload"),
            e,
        )
    })
}

/// The approval bound on an email scope: at most 20 entries per list, at least
/// one matcher, each 2..=200 characters after trimming.
pub fn validate_email_scope(scope: &WorkspaceEmailScope) -> Result<(), WorkspaceError> {
    let lists = [
        &scope.senders,
        &scope.domains,
        &scope.subject_keywords,
        &scope.body_keywords,
    ];
    let mut matchers = lists.iter().flat_map(|list| list.iter()).peekable();
    let empty = matchers.peek().is_none();
    let unbounded = matchers.any(|v| js_len(js_trim(v)) < 2 || js_len(v) > 200);
    if lists.iter().any(|list| list.len() > 20) || empty || unbounded {
        return Err(WorkspaceError::validation(
            "An email scope must contain bounded, non-empty matchers",
        ));
    }
    Ok(())
}

impl WorkspaceService {
    /// Claims `action_id` for resolution; `None` when another resolution holds it.
    fn claim_resolution(&self, action_id: &str) -> Option<Resolving> {
        let inserted = self
            .inner
            .resolving
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(action_id.to_owned());
        inserted.then(|| Resolving {
            service: self.clone(),
            action_id: action_id.to_owned(),
        })
    }

    async fn finish(
        &self,
        action_id: &str,
        status: WorkspaceActionStatus,
        result: &str,
    ) -> Result<ActionRow, WorkspaceError> {
        self.repo()
            .set_action_result(action_id, status, result)
            .await
            .map_err(op("resolve workspace action"))?
            .ok_or_else(|| {
                WorkspaceError::action(action_id, "Workspace action disappeared while resolving it")
            })
    }

    /// Approves and executes one pending or failed proposal. Runs on the app
    /// tracker, so dropping the caller never splits the external effect from
    /// its durable record.
    pub async fn approve_action(&self, action_id: &str) -> Result<ActionRow, WorkspaceError> {
        let service = self.clone();
        let action_id = action_id.to_owned();
        let tracker = self.inner.tracker.clone();
        omni_core::spawn::must_complete(&tracker, async move {
            service.approve_claimed(&action_id).await
        })
        .await
    }

    async fn approve_claimed(&self, action_id: &str) -> Result<ActionRow, WorkspaceError> {
        let Some(_claim) = self.claim_resolution(action_id) else {
            return Err(WorkspaceError::action(
                action_id,
                "Workspace action is already being resolved",
            ));
        };
        let action = self
            .repo()
            .get_action(action_id)
            .await
            .map_err(op("read workspace action"))?
            .ok_or_else(|| WorkspaceError::action(action_id, "Workspace action not found"))?;
        if !matches!(
            action.status,
            WorkspaceActionStatus::Pending | WorkspaceActionStatus::Failed
        ) {
            return Err(WorkspaceError::action(
                action_id,
                format!("Workspace action is already {}", action.status.as_str()),
            ));
        }
        match self.execute_approval(&action).await {
            Ok(row) => Ok(row),
            Err(error) => {
                if let Err(persist) = self
                    .repo()
                    .set_action_result(action_id, WorkspaceActionStatus::Failed, &error.to_string())
                    .await
                {
                    tracing::error!(
                        target: LOG,
                        error = %persist,
                        "Failed to record workspace action {action_id} failure"
                    );
                }
                Err(error)
            }
        }
    }

    async fn execute_approval(&self, action: &ActionRow) -> Result<ActionRow, WorkspaceError> {
        let action_id = action.action_id.as_str();
        match action.action_type {
            WorkspaceActionType::EmailScope => {
                let scope: WorkspaceEmailScope = decode_payload(action_id, &action.payload)?;
                validate_email_scope(&scope)?;
                self.repo()
                    .upsert_email_scope(&action.workspace_id, &action.subject_id, scope)
                    .await
                    .map_err(op("enable workspace email scope"))?;
                self.finish(
                    action_id,
                    WorkspaceActionStatus::Approved,
                    "Email scope enabled",
                )
                .await
            }
            WorkspaceActionType::CalendarEvent => {
                let event: CalendarEventInput = decode_payload(action_id, &action.payload)?;
                if js_trim(&event.title).is_empty() || js_trim(&event.start_date).is_empty() {
                    return Err(WorkspaceError::validation(
                        "A calendar event requires a title and start date",
                    ));
                }
                let writer = self.inner.ports.calendar_writer().ok_or_else(|| {
                    WorkspaceError::action(
                        action_id,
                        PortError::Unavailable("Calendar writer").to_string(),
                    )
                })?;
                let outcome = writer
                    .create_event(&calendar_uid(action_id), &event)
                    .await
                    .map_err(|e| WorkspaceError::Action {
                        action_id: action_id.to_owned(),
                        message: e.to_string(),
                        source: None,
                    })?;
                let result = match outcome {
                    CalendarCreateOutcome::Created { event_uid } => {
                        format!("Calendar event created ({event_uid})")
                    }
                    CalendarCreateOutcome::AlreadyExists => {
                        "Calendar event was already created".to_owned()
                    }
                };
                self.finish(action_id, WorkspaceActionStatus::Approved, &result)
                    .await
            }
        }
    }

    /// Rejects one pending proposal without performing its effect.
    pub async fn reject_action(&self, action_id: &str) -> Result<ActionRow, WorkspaceError> {
        let Some(_claim) = self.claim_resolution(action_id) else {
            return Err(WorkspaceError::action(
                action_id,
                "Workspace action is already being resolved",
            ));
        };
        let action = self
            .repo()
            .get_action(action_id)
            .await
            .map_err(op("read workspace action"))?
            .ok_or_else(|| WorkspaceError::action(action_id, "Workspace action not found"))?;
        if action.status != WorkspaceActionStatus::Pending {
            return Err(WorkspaceError::action(
                action_id,
                format!("Workspace action is already {}", action.status.as_str()),
            ));
        }
        self.finish(
            action_id,
            WorkspaceActionStatus::Rejected,
            "Rejected by user",
        )
        .await
    }
}
