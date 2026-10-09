//! `CastroInboxCleanup`: every six hours, clear Substack free-preview episodes
//! from the Castro Inbox (`castro/inboxCleanupTask.ts`). Only `clear_episode_new`
//! is posted; the queue is never read or changed. An unavailable Inbox fails
//! the run instead of counting as empty.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::account::{AccountProvider, InboxEpisode, PodcastWriteResult};

const LOG: &str = "CastroInboxCleanup";
pub const TASK_NAME: &str = "CastroInboxCleanup";
pub const SCHEDULE: &str = "0 */6 * * *";
pub const FREE_PREVIEW_DESCRIPTION_PREFIX: &str = "This is a free preview";

/// Only descriptions that START with the exact Substack preview text match.
pub fn is_free_preview_episode(episode: &InboxEpisode) -> bool {
    episode
        .description
        .as_deref()
        .is_some_and(|d| d.starts_with(FREE_PREVIEW_DESCRIPTION_PREFIX))
}

/// `CastroInboxCleanupError`: `<operation>: <detail>`.
#[derive(Debug, thiserror::Error)]
#[error("{operation}: {detail}")]
pub struct CastroInboxCleanupError {
    pub operation: &'static str,
    pub detail: String,
}

pub struct CastroInboxCleanupTask {
    schedule: CronSchedule,
    accounts: Arc<dyn AccountProvider>,
    last_summary: Mutex<Option<String>>,
}

impl CastroInboxCleanupTask {
    pub fn new(schedule: CronSchedule, accounts: Arc<dyn AccountProvider>) -> Self {
        Self {
            schedule,
            accounts,
            last_summary: Mutex::new(None),
        }
    }

    fn set_summary(&self, summary: Option<String>) {
        *self.last_summary.lock().unwrap_or_else(|p| p.into_inner()) = summary;
    }

    /// One cleanup pass.
    pub async fn run_once(&self) -> Result<(), CastroInboxCleanupError> {
        self.set_summary(None);
        let Some(account) = self.accounts.resolve() else {
            return Err(CastroInboxCleanupError {
                operation: "create account client",
                detail: "Castro account is not configured".to_owned(),
            });
        };
        let inbox = account
            .fetch_inbox()
            .await
            .map_err(|e| CastroInboxCleanupError {
                operation: "fetch inbox",
                detail: format!("Castro inbox unavailable: {}", e.reason),
            })?;
        let mut removed = 0usize;
        for episode in inbox.iter().filter(|e| is_free_preview_episode(e)) {
            match account
                .clear_inbox_episode(&episode.client_episode_id)
                .await
            {
                PodcastWriteResult::Removed => {
                    tracing::info!(
                        target: LOG,
                        "Cleared free preview from Castro inbox: {} - {}",
                        episode.show_title,
                        episode.episode_title
                    );
                    removed += 1;
                }
                PodcastWriteResult::NotFound => {}
                other => {
                    return Err(CastroInboxCleanupError {
                        operation: "clear inbox episode",
                        detail: format!(
                            "Could not clear Castro preview episode ({}): {}",
                            other.as_str(),
                            episode.episode_title
                        ),
                    });
                }
            }
        }
        let summary = format!("cleared {removed} free preview episode(s) from inbox");
        tracing::info!(target: LOG, "Castro inbox cleanup finished: {summary}");
        self.set_summary(Some(summary));
        Ok(())
    }
}

impl Task for CastroInboxCleanupTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        // Drift off the exact top of the hour to avoid an obvious automated pattern.
        TaskOptions {
            jitter: Duration::from_secs(5 * 60),
            run_on_startup: false,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move { self.run_once().await.map_err(TaskError::from_error) })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
