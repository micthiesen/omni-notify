//! `PodcastTasteReflection` task: the weekly deep read
//! of the full listen history (the client caps at 180 days).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use omni_store::Store;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use super::core::{
    DEFAULT_MAX_EVIDENCE, PodcastTasteReflectionInput, PodcastTasteReflectionResult,
    run_podcast_taste_reflection,
};
use crate::account::AccountProvider;
use crate::models::Models;
use crate::persistence::get_all_podcast_recommendations;

const LOG: &str = "PodcastTasteReflection";
pub const TASK_NAME: &str = "PodcastTasteReflection";

pub struct PodcastTasteReflectionTask {
    schedule: CronSchedule,
    store: Store,
    clock: SharedClock,
    models: Models,
    accounts: Arc<dyn AccountProvider>,
    last_summary: Mutex<Option<String>>,
}

impl PodcastTasteReflectionTask {
    pub fn new(
        schedule: CronSchedule,
        store: Store,
        clock: SharedClock,
        models: Models,
        accounts: Arc<dyn AccountProvider>,
    ) -> Self {
        Self {
            schedule,
            store,
            clock,
            models,
            accounts,
            last_summary: Mutex::new(None),
        }
    }

    fn set_summary(&self, summary: String) {
        *self.last_summary.lock().unwrap_or_else(|p| p.into_inner()) = Some(summary);
    }

    pub async fn run_once(&self) -> Result<(), TaskError> {
        let Some(account) = self.accounts.resolve() else {
            self.set_summary("skipped: no podcast account client".to_owned());
            tracing::warn!(target: LOG, "Podcast taste reflection skipped: no account client");
            return Ok(());
        };
        let history = match account.fetch_listen_history(None).await {
            Ok(history) => history,
            Err(e) => {
                self.set_summary(format!("skipped: {}", e.reason));
                tracing::warn!(target: LOG, "Podcast taste reflection skipped: {}", e.reason);
                return Ok(());
            }
        };
        let recommendations = get_all_podcast_recommendations(&self.store)
            .await
            .map_err(TaskError::from_error)?;
        let result = run_podcast_taste_reflection(
            &self.store,
            &self.models,
            PodcastTasteReflectionInput {
                listened: history,
                recommendations,
                now: self.clock.now_ms(),
                max_evidence: DEFAULT_MAX_EVIDENCE,
            },
        )
        .await
        .map_err(TaskError::from_error)?;
        let summary = match result {
            PodcastTasteReflectionResult::Created {
                profile,
                rejected_claims,
                ..
            } => format!(
                "profile v{}: {} evidence items, {rejected_claims} unsupported claims removed",
                profile.version, profile.evidence_count
            ),
            PodcastTasteReflectionResult::Unchanged { profile, .. } => {
                format!("unchanged: profile v{}, no model call", profile.version)
            }
            PodcastTasteReflectionResult::InsufficientEvidence { .. } => {
                "no listen or recommendation evidence".to_owned()
            }
        };
        tracing::info!(target: LOG, "Podcast taste reflection finished: {summary}");
        self.set_summary(summary);
        Ok(())
    }
}

impl Task for PodcastTasteReflectionTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn display_name(&self) -> Option<&str> {
        Some("Podcast Taste Reflection")
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions {
            jitter: Duration::from_secs(5 * 60),
            run_on_startup: false,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(self.run_once())
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
