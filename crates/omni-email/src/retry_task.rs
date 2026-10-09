//! `EmailRetry`: replays transiently failed email
//! processing. Each due row is claimed (attempt counted) before any network
//! or handler work; the email is re-fetched by id and the owning pipeline's
//! handler rerun. A resolved handler is not proof of success: the row is
//! cleared only when the run did not enqueue the email again.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_runtime::Ports;
use omni_store::{Store, StoreError};
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::retry::{self, EmailRetryData, MAX_RETRY_ATTEMPTS};

const LOG: &str = "Main:EmailRetry";
pub const NAME: &str = "EmailRetry";
/// Every 15 minutes.
pub const SCHEDULE: &str = "0 */15 * * * *";

pub struct EmailRetryTask {
    store: Store,
    ports: Ports,
    schedule: CronSchedule,
    summary: Mutex<Option<String>>,
}

#[derive(Default)]
struct Tally {
    succeeded: usize,
    requeued: usize,
    exhausted: usize,
    missing: usize,
    orphaned: usize,
    permanent: usize,
}

impl EmailRetryTask {
    /// The task no-ops ("pipelines not connected") until the `EmailReader` and
    /// `EmailRetryHandlers` ports are set, so it survives a failed connect at boot.
    pub fn new(store: Store, ports: Ports, schedule: CronSchedule) -> Self {
        Self {
            store,
            ports,
            schedule,
            summary: Mutex::new(None),
        }
    }

    fn set_summary(&self, summary: String) {
        *self.summary.lock().unwrap_or_else(|p| p.into_inner()) = Some(summary);
    }

    /// One retry pass.
    pub async fn run_pass(&self) -> Result<(), StoreError> {
        let rows = retry::get_all(&self.store).await?;
        let stale_exhausted: Vec<&EmailRetryData> = rows
            .iter()
            .filter(|r| r.attempts >= MAX_RETRY_ATTEMPTS)
            .collect();
        for row in &stale_exhausted {
            retry::clear(&self.store, &row.pipeline, &row.email_id).await?;
        }
        let due = retry::select_due(&rows, self.store.clock().now_ms());
        if due.is_empty() {
            tracing::debug!(target: LOG, "No email retries due");
            self.set_summary("No retries due".to_owned());
            return Ok(());
        }

        let (Some(reader), Some(handlers)) =
            (self.ports.email_reader(), self.ports.email_retry_handlers())
        else {
            self.set_summary(format!("{} due, pipelines not connected yet", due.len()));
            tracing::info!(
                target: LOG,
                "{} retry(ies) due but the email pipelines are not connected; deferring",
                due.len()
            );
            return Ok(());
        };

        let mut tally = Tally::default();
        for row in &due {
            let claimed = retry::claim(&self.store, row).await?;
            let Some(handler) = handlers.handler(&row.pipeline) else {
                tracing::warn!(
                    target: LOG,
                    "No handler registered for pipeline \"{}\"; dropping retry for email {}",
                    row.pipeline,
                    row.email_id
                );
                retry::clear(&self.store, &row.pipeline, &row.email_id).await?;
                tally.orphaned += 1;
                continue;
            };

            let email = match reader.fetch_by_id(&row.email_id, false).await {
                Err(error) => {
                    if claimed.attempts >= MAX_RETRY_ATTEMPTS {
                        retry::clear(&self.store, &row.pipeline, &row.email_id).await?;
                        tally.exhausted += 1;
                    } else {
                        tally.requeued += 1;
                    }
                    tracing::warn!(
                        target: LOG,
                        "Could not fetch {} email {} (attempt {}/{MAX_RETRY_ATTEMPTS}): {error}",
                        row.pipeline,
                        row.email_id,
                        claimed.attempts
                    );
                    continue;
                }
                Ok(None) => {
                    tracing::info!(
                        target: LOG,
                        "Email {} no longer exists; dropping {} retry",
                        row.email_id,
                        row.pipeline
                    );
                    retry::clear(&self.store, &row.pipeline, &row.email_id).await?;
                    tally.missing += 1;
                    continue;
                }
                Ok(Some(email)) => email,
            };

            match handler.handle(std::slice::from_ref(&email)).await {
                Ok(()) => {
                    let after = retry::get(&self.store, &row.retry_key).await?;
                    let re_enqueued = after.as_ref().filter(|after| {
                        after.enqueue_count.unwrap_or(0) > row.enqueue_count.unwrap_or(0)
                    });
                    if let Some(after) = re_enqueued {
                        if claimed.attempts >= MAX_RETRY_ATTEMPTS {
                            retry::clear(&self.store, &row.pipeline, &row.email_id).await?;
                            tally.exhausted += 1;
                            tracing::warn!(
                                target: LOG,
                                "Giving up on {} email \"{}\" after {} attempts: {}",
                                row.pipeline,
                                email.subject,
                                claimed.attempts,
                                after.reason
                            );
                        } else {
                            tally.requeued += 1;
                            tracing::info!(
                                target: LOG,
                                "Retry failed again for {} email \"{}\" (attempt {}/{MAX_RETRY_ATTEMPTS}): {}",
                                row.pipeline,
                                email.subject,
                                claimed.attempts,
                                after.reason
                            );
                        }
                        continue;
                    }
                    retry::clear(&self.store, &row.pipeline, &row.email_id).await?;
                    tally.succeeded += 1;
                    tracing::info!(
                        target: LOG,
                        "Retry succeeded for {} email \"{}\" (attempt {})",
                        row.pipeline,
                        email.subject,
                        claimed.attempts
                    );
                }
                Err(error) => {
                    retry::clear(&self.store, &row.pipeline, &row.email_id).await?;
                    tally.permanent += 1;
                    tracing::warn!(
                        target: LOG,
                        "Dropping permanent {} retry for email \"{}\": {error}",
                        row.pipeline,
                        email.subject
                    );
                }
            }
        }

        let orphaned = if tally.orphaned > 0 {
            format!(", {} orphaned", tally.orphaned)
        } else {
            String::new()
        };
        let permanent = if tally.permanent > 0 {
            format!(", {} permanent", tally.permanent)
        } else {
            String::new()
        };
        let summary = format!(
            "{} due, {} succeeded, {} requeued, {} exhausted, {} missing{orphaned}{permanent}",
            due.len(),
            tally.succeeded,
            tally.requeued,
            tally.exhausted + stale_exhausted.len(),
            tally.missing
        );
        tracing::info!(target: LOG, "Email retry pass: {summary}");
        self.set_summary(summary);
        Ok(())
    }
}

impl Task for EmailRetryTask {
    fn name(&self) -> &str {
        NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions::default()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move { self.run_pass().await.map_err(TaskError::from_error) })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// Builds the task with its production schedule.
pub fn task(
    store: Store,
    ports: Ports,
    tz: &jiff::tz::TimeZone,
) -> Result<Arc<dyn Task>, omni_tasks::InvalidScheduleError> {
    Ok(Arc::new(EmailRetryTask::new(
        store,
        ports,
        CronSchedule::parse(SCHEDULE, tz)?,
    )))
}
