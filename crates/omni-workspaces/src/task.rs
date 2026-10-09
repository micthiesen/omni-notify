//! `WorkspaceTask` (`src/workspaces/task.ts`): scheduled refreshes of active
//! subjects, and manual runs for user messages and scoped emails.

use std::sync::Mutex;

use futures::future::BoxFuture;
use omni_api::workspaces::{WorkspaceDefinition, WorkspaceSubjectStatus};
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};
use serde::Deserialize;

use crate::definitions::scheduled_runs;
use crate::engine::{RunRequest, RunTrigger};
use crate::error::{WorkspaceError, op};
use crate::service::WorkspaceService;
use crate::text::{js_len, js_trim};

const LOG: &str = "Workspaces";

/// Manual-run trigger accepted in task input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ManualTrigger {
    Message,
    Email,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualInput {
    message: String,
    #[serde(default)]
    subject_id: Option<String>,
    #[serde(default)]
    trigger: Option<ManualTrigger>,
}

/// One workspace's task (`PurchaseResearch`, `MarketplaceSelling`).
pub struct WorkspaceTask {
    service: WorkspaceService,
    definition: WorkspaceDefinition,
    schedule: CronSchedule,
    last_summary: Mutex<Option<String>>,
}

impl WorkspaceTask {
    pub fn new(
        service: WorkspaceService,
        definition: WorkspaceDefinition,
        schedule: CronSchedule,
    ) -> Self {
        Self {
            service,
            definition,
            schedule,
            last_summary: Mutex::new(None),
        }
    }

    fn set_summary(&self, summary: String) {
        *self.last_summary.lock().unwrap_or_else(|p| p.into_inner()) = Some(summary);
    }

    /// Scheduled refresh: one subject-scoped run per active subject, in order.
    pub async fn run_scheduled(&self, run_id: Option<&str>) -> Result<(), WorkspaceError> {
        if !scheduled_runs(&self.definition) {
            self.set_summary("On-demand workspace; scheduled refresh skipped".to_owned());
            return Ok(());
        }
        let subjects: Vec<_> = self
            .service
            .repo()
            .list_subjects(&self.definition.id)
            .await
            .map_err(op("list active workspace subjects"))?
            .into_iter()
            .filter(|s| s.status == WorkspaceSubjectStatus::Active)
            .collect();
        if subjects.is_empty() {
            self.set_summary("No active subjects to research".to_owned());
            return Ok(());
        }
        let (mut updated, mut actions) = (0, 0);
        let mut failures = Vec::new();
        for subject in &subjects {
            let request = RunRequest {
                trigger: RunTrigger::Scheduled,
                message: None,
                subject_id: Some(subject.subject_id.clone()),
            };
            match self
                .service
                .run_workspace(&self.definition, &request, run_id)
                .await
            {
                Ok(result) => {
                    updated += result.updated_subjects;
                    actions += result.created_actions;
                }
                Err(error) => {
                    tracing::warn!(
                        target: LOG,
                        error = %error,
                        "Workspace research failed for {}",
                        subject.title
                    );
                    failures.push(format!("{}: {error}", subject.title));
                }
            }
        }
        let summary = format!(
            "Updated {updated} subject(s), proposed {actions} action(s), {} failed",
            failures.len()
        );
        self.set_summary(summary.clone());
        if failures.is_empty() {
            Ok(())
        } else {
            Err(WorkspaceError::validation(format!(
                "{summary}: {}",
                failures.join("; ")
            )))
        }
    }

    /// Manual run from `{message, subjectId?, trigger?}`.
    pub async fn run_with_input(
        &self,
        input: serde_json::Value,
        run_id: Option<&str>,
    ) -> Result<(), WorkspaceError> {
        let parsed: ManualInput = serde_json::from_value(input)
            .map_err(|e| WorkspaceError::validation_with("Invalid workspace manual input", e))?;
        let message = js_trim(&parsed.message);
        if message.is_empty()
            || js_len(message) > 20_000
            || parsed.subject_id.as_deref() == Some("")
        {
            return Err(WorkspaceError::validation(
                "Workspace manual input is empty or too long",
            ));
        }
        let request = RunRequest {
            trigger: match parsed.trigger {
                Some(ManualTrigger::Email) => RunTrigger::Email,
                Some(ManualTrigger::Message) | None => RunTrigger::Message,
            },
            message: Some(message.to_owned()),
            subject_id: parsed.subject_id,
        };
        let result = self
            .service
            .run_workspace(&self.definition, &request, run_id)
            .await?;
        self.set_summary(result.summary);
        Ok(())
    }
}

impl Task for WorkspaceTask {
    fn name(&self) -> &str {
        &self.definition.task_name
    }

    fn display_name(&self) -> Option<&str> {
        Some(&self.definition.title)
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions::default()
    }

    fn run<'a>(&'a self, cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            self.run_scheduled(Some(&cx.run_id))
                .await
                .map_err(TaskError::from_error)
        })
    }

    fn accepts_manual_input(&self) -> bool {
        true
    }

    fn run_manual<'a>(
        &'a self,
        cx: &'a RunContext,
        input: serde_json::Value,
    ) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            self.run_with_input(input, Some(&cx.run_id))
                .await
                .map_err(TaskError::from_error)
        })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
