//! Scoped email ingestion: emails matching an
//! approved scope of an active subject become sources (persisted before the
//! workspace run is triggered) and trigger one email run per subject.
//! `triggeredAt` is set only after that run succeeded, so a failed run is
//! retried when the batch replays.

use std::sync::Arc;

use futures::future::BoxFuture;
use indexmap::IndexMap;
use omni_api::workspaces::{WorkspaceSourceKind, WorkspaceSubjectStatus};
use omni_core::email::{EmailHandler, FetchedEmail, HandlerError};
use omni_tasks::{RunNowError, TaskRegistry};
use serde_json::json;

use crate::entities::EmailScopeRow;
use crate::error::{WorkspaceError, op};
use crate::persistence::{NewSource, WorkspaceRepo};
use crate::text::{js_prefix, js_trim};

const LOG: &str = "Workspaces";

/// Requests an email-triggered workspace run and waits for it to finish.
pub trait EmailRunTrigger: Send + Sync {
    fn trigger<'a>(
        &'a self,
        workspace_id: &'a str,
        subject_id: &'a str,
        message: String,
    ) -> BoxFuture<'a, Result<(), WorkspaceError>>;
}

/// Production trigger: `run_now_and_wait` on the workspace's task with
/// `{message, subjectId, trigger: "email"}`.
pub struct RegistryEmailTrigger {
    pub tasks: TaskRegistry,
    /// `(workspace id, task name)` pairs.
    pub workspaces: Vec<(String, String)>,
}

impl EmailRunTrigger for RegistryEmailTrigger {
    fn trigger<'a>(
        &'a self,
        workspace_id: &'a str,
        subject_id: &'a str,
        message: String,
    ) -> BoxFuture<'a, Result<(), WorkspaceError>> {
        Box::pin(async move {
            let Some((_, task_name)) = self.workspaces.iter().find(|(id, _)| id == workspace_id)
            else {
                return Err(WorkspaceError::operation(
                    "resolve email-triggered workspace",
                    format!("Unknown workspace {workspace_id}"),
                ));
            };
            let input = json!({ "message": message, "subjectId": subject_id, "trigger": "email" });
            self.tasks
                .run_now_and_wait(task_name, Some(input))
                .await
                .map_err(|e: RunNowError| {
                    WorkspaceError::operation("run email-triggered workspace", e)
                })?;
            tracing::info!(
                target: LOG,
                "Completed workspace email run for {workspace_id}/{subject_id}"
            );
            Ok(())
        })
    }
}

/// `(from.match(/<([^>]+)>/)?.[1] ?? from).trim()`.
fn extract_address(from: &str) -> &str {
    let bracketed = from.match_indices('<').find_map(|(start, _)| {
        let rest = &from[start + 1..];
        rest.find('>')
            .filter(|&end| end > 0)
            .map(|end| &rest[..end])
    });
    js_trim(bracketed.unwrap_or(from))
}

/// Exact sender address, domain (optional leading
/// `@`), or case-insensitive subject/body keyword.
pub fn matches_workspace_email(email: &FetchedEmail, scope: &EmailScopeRow) -> bool {
    let from = js_trim(&email.from).to_lowercase();
    let address = extract_address(&from);
    let domain = address.split('@').nth(1).unwrap_or("");
    let subject = email.subject.to_lowercase();
    let body = email.text_body.to_lowercase();
    scope
        .senders
        .iter()
        .any(|v| address == js_trim(v).to_lowercase())
        || scope.domains.iter().any(|v| {
            let normalized = js_trim(v).to_lowercase();
            domain == normalized.strip_prefix('@').unwrap_or(&normalized)
        })
        || scope
            .subject_keywords
            .iter()
            .any(|v| subject.contains(&v.to_lowercase()))
        || scope
            .body_keywords
            .iter()
            .any(|v| body.contains(&v.to_lowercase()))
}

/// The `Workspaces` email handler (registered last by app wiring).
pub struct WorkspaceEmailHandler {
    repo: WorkspaceRepo,
    trigger: Arc<dyn EmailRunTrigger>,
}

impl WorkspaceEmailHandler {
    pub fn new(repo: WorkspaceRepo, trigger: Arc<dyn EmailRunTrigger>) -> Self {
        Self { repo, trigger }
    }

    pub async fn handle_emails(&self, emails: &[FetchedEmail]) -> Result<(), WorkspaceError> {
        let scopes = self
            .repo
            .list_all_email_scopes()
            .await
            .map_err(op("list workspace email scopes"))?;
        let mut matches: IndexMap<(String, String), Vec<(&FetchedEmail, String)>> = IndexMap::new();
        for email in emails {
            for scope in &scopes {
                if !matches_workspace_email(email, scope) {
                    continue;
                }
                let subject = self
                    .repo
                    .get_subject(&scope.workspace_id, &scope.subject_id)
                    .await
                    .map_err(op("read workspace email subject"))?;
                if subject.map(|s| s.status) != Some(WorkspaceSubjectStatus::Active) {
                    continue;
                }
                let source_id = format!(
                    "email:{}:{}:{}",
                    scope.workspace_id, scope.subject_id, email.id
                );
                let existing = self
                    .repo
                    .get_source(&source_id)
                    .await
                    .map_err(op("read workspace email source"))?;
                if existing.as_ref().is_some_and(|s| s.triggered_at.is_some()) {
                    continue;
                }
                matches
                    .entry((scope.workspace_id.clone(), scope.subject_id.clone()))
                    .or_default()
                    .push((email, source_id.clone()));
                if existing.is_none() {
                    let title = if email.subject.is_empty() {
                        format!("(Email from {})", email.from)
                    } else {
                        email.subject.clone()
                    };
                    self.repo
                        .add_source(NewSource {
                            source_id: Some(source_id),
                            workspace_id: scope.workspace_id.clone(),
                            subject_id: scope.subject_id.clone(),
                            kind: WorkspaceSourceKind::Email,
                            title,
                            url: None,
                            excerpt: js_prefix(&email.text_body, 4_000),
                            email_id: Some(email.id.clone()),
                            run_id: None,
                        })
                        .await
                        .map_err(op("persist workspace email source"))?;
                }
            }
        }
        for ((workspace_id, subject_id), matched) in matches {
            tracing::info!(
                target: LOG,
                "Ingested {} scoped email(s) for {workspace_id}/{subject_id}",
                matched.len()
            );
            let subjects = matched
                .iter()
                .map(|(email, _)| email.subject.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            self.trigger
                .trigger(
                    &workspace_id,
                    &subject_id,
                    format!(
                        "Review {} newly ingested scoped email(s): {subjects}",
                        matched.len()
                    ),
                )
                .await?;
            self.repo
                .mark_sources_triggered(matched.into_iter().map(|(_, id)| id).collect())
                .await
                .map_err(op("mark workspace email sources triggered"))?;
        }
        Ok(())
    }
}

impl EmailHandler for WorkspaceEmailHandler {
    fn name(&self) -> &'static str {
        "Workspaces"
    }

    fn handle<'a>(&'a self, emails: &'a [FetchedEmail]) -> BoxFuture<'a, Result<(), HandlerError>> {
        Box::pin(async move {
            self.handle_emails(emails).await.map_err(|error| {
                // Replay is safe: sources are deduplicated by id and only
                // marked triggered after their run succeeded.
                HandlerError::transient(error.to_string(), Some(Box::new(error)))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bracketed_addresses() {
        assert_eq!(
            extract_address("framework <orders@frame.work>"),
            "orders@frame.work"
        );
        assert_eq!(
            extract_address("alerts@shop.example"),
            "alerts@shop.example"
        );
        assert_eq!(extract_address(" a <> b "), "a <> b");
        assert_eq!(extract_address("a <> <b@c>"), "b@c");
    }
}
