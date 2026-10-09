//! The durable workspace notification outbox (`src/workspaces/notifications.ts`).
//!
//! Each provider attempt is reserved (`sending`) before Pushover is called. A
//! row still `sending` when found again had an unacknowledged attempt and is
//! marked `unknown`, never resent. Failures back off up to six hours.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use futures::future::BoxFuture;
use omni_alerts::{Pushover, PushoverChannel, PushoverMessage};
use omni_store::StoreError;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::entities::{NotificationRow, NotificationStatus};
use crate::error::{WorkspaceError, op};
use crate::persistence::WorkspaceRepo;

const LOG: &str = "WorkspaceNotificationTask";
/// Concurrent deliveries per batch.
pub const DELIVERY_CONCURRENCY: usize = 4;
/// Rows processed per task run.
pub const DUE_LIMIT: usize = 20;

/// Sends one workspace push (Pushover `PUSHOVER_WORKSPACE_TOKEN` in production).
pub trait WorkspaceNotifier: Send + Sync {
    /// `Err` carries the provider failure message.
    fn send<'a>(&'a self, notification: &'a NotificationRow) -> BoxFuture<'a, Result<(), String>>;
}

/// Pushover on the workspace channel (honors `SideEffectMode::Record`).
pub struct PushoverWorkspaceNotifier(pub Pushover);

impl WorkspaceNotifier for PushoverWorkspaceNotifier {
    fn send<'a>(&'a self, n: &'a NotificationRow) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let message = PushoverMessage {
                message: n.message.clone(),
                title: Some(n.title.clone()),
                url: Some(n.url.clone()),
                url_title: Some(n.url_title.clone()),
                ..PushoverMessage::default()
            };
            self.0
                .send(PushoverChannel::Workspace, message)
                .await
                .map(drop)
                .map_err(|e| pushover_error_message(&e))
        })
    }
}

/// mitools `PushoverError.message`, which TS persists as `lastError`.
pub fn pushover_error_message(error: &omni_alerts::PushoverError) -> String {
    match error.status {
        Some(status) => format!("Pushover API returned status code {status}: {}", error.body),
        None => format!("Pushover request failed: {}", error.body),
    }
}

/// The outbox state transitions (the store in production; a seam for tests).
pub trait NotificationOutbox: Send + Sync {
    fn mark_sending<'a>(
        &'a self,
        id: &'a str,
        attempts: i64,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
    fn mark_sent<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), StoreError>>;
    fn mark_unknown<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), StoreError>>;
    fn mark_failed<'a>(
        &'a self,
        id: &'a str,
        attempts: i64,
        error: String,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
}

impl NotificationOutbox for WorkspaceRepo {
    fn mark_sending<'a>(
        &'a self,
        id: &'a str,
        attempts: i64,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(self.mark_notification_sending(id, attempts))
    }
    fn mark_sent<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(self.mark_notification_sent(id))
    }
    fn mark_unknown<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(self.mark_notification_unknown(id))
    }
    fn mark_failed<'a>(
        &'a self,
        id: &'a str,
        attempts: i64,
        error: String,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(self.mark_notification_failed(id, attempts, error))
    }
}

/// Delivers outbox rows with in-process mutual exclusion per row.
pub struct NotificationDelivery {
    outbox: Arc<dyn NotificationOutbox>,
    notifier: Arc<dyn WorkspaceNotifier>,
    delivering: Mutex<HashSet<String>>,
}

/// Releases a row's in-process claim on every exit path.
struct Claim<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.id);
    }
}

impl NotificationDelivery {
    pub fn new(outbox: Arc<dyn NotificationOutbox>, notifier: Arc<dyn WorkspaceNotifier>) -> Self {
        Self {
            outbox,
            notifier,
            delivering: Mutex::new(HashSet::new()),
        }
    }

    fn claim(&self, id: &str) -> Option<Claim<'_>> {
        let inserted = self
            .delivering
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_owned());
        inserted.then(|| Claim {
            set: &self.delivering,
            id: id.to_owned(),
        })
    }

    /// `true` when the row is settled (sent now, already sent, or acknowledged
    /// as unknown); `false` when it stays queued or another delivery holds it.
    pub async fn deliver(&self, notification: &NotificationRow) -> Result<bool, WorkspaceError> {
        let id = notification.notification_id.as_str();
        if matches!(
            notification.status,
            NotificationStatus::Sent | NotificationStatus::Unknown
        ) {
            return Ok(true);
        }
        let Some(_claim) = self.claim(id) else {
            return Ok(false);
        };
        if notification.status == NotificationStatus::Sending {
            tracing::warn!(
                target: LOG,
                "Workspace notification {id} had an unacknowledged provider attempt; acknowledging without resending"
            );
            self.outbox
                .mark_unknown(id)
                .await
                .map_err(op("acknowledge reserved workspace notification"))?;
            return Ok(true);
        }
        let attempts = notification.attempts + 1;
        self.outbox
            .mark_sending(id, attempts)
            .await
            .map_err(op("reserve workspace notification delivery"))?;
        match self.notifier.send(notification).await {
            Ok(()) => {
                self.outbox
                    .mark_sent(id)
                    .await
                    .map_err(op("mark workspace notification sent"))?;
                Ok(true)
            }
            Err(error) => {
                let message = format!("send workspace notification failed: {error}");
                self.outbox
                    .mark_failed(id, attempts, message.clone())
                    .await
                    .map_err(op("record workspace notification provider failure"))?;
                tracing::warn!(
                    target: LOG,
                    error = %message,
                    "Workspace notification {id} failed (attempt {attempts}); queued for retry"
                );
                Ok(false)
            }
        }
    }

    /// Delivers up to [`DELIVERY_CONCURRENCY`] rows at a time; returns how many settled.
    pub async fn deliver_all(&self, rows: &[NotificationRow]) -> Result<usize, WorkspaceError> {
        let deliveries: Vec<BoxFuture<'_, Result<bool, WorkspaceError>>> = rows
            .iter()
            .map(|row| Box::pin(self.deliver(row)) as BoxFuture<'_, _>)
            .collect();
        let results: Vec<Result<bool, WorkspaceError>> = futures::stream::iter(deliveries)
            .buffered(DELIVERY_CONCURRENCY)
            .collect()
            .await;
        let mut sent = 0;
        for result in results {
            if result? {
                sent += 1;
            }
        }
        Ok(sent)
    }
}

/// `WorkspaceNotifications`: drains due outbox rows every five minutes.
pub struct WorkspaceNotificationTask {
    repo: WorkspaceRepo,
    delivery: Arc<NotificationDelivery>,
    schedule: CronSchedule,
    last_summary: Mutex<Option<String>>,
}

impl WorkspaceNotificationTask {
    pub const NAME: &'static str = "WorkspaceNotifications";
    pub const SCHEDULE: &'static str = "*/5 * * * *";

    pub fn new(
        repo: WorkspaceRepo,
        delivery: Arc<NotificationDelivery>,
        schedule: CronSchedule,
    ) -> Self {
        Self {
            repo,
            delivery,
            schedule,
            last_summary: Mutex::new(None),
        }
    }

    async fn execute(&self) -> Result<(), WorkspaceError> {
        let due = self
            .repo
            .list_due_notifications(self.repo.now_ms(), DUE_LIMIT)
            .await
            .map_err(op("list due workspace notifications"))?;
        let sent = self.delivery.deliver_all(&due).await?;
        *self.last_summary.lock().unwrap_or_else(|p| p.into_inner()) = Some(format!(
            "Sent {sent} notification(s); {} queued for retry",
            due.len() - sent
        ));
        Ok(())
    }
}

impl Task for WorkspaceNotificationTask {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn display_name(&self) -> Option<&str> {
        Some("Workspace Notifications")
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions::default()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move { self.execute().await.map_err(TaskError::from_error) })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_failures_read_like_mitools() {
        let rejected = omni_alerts::PushoverError {
            status: Some(400),
            body: "{\"status\":0}".to_owned(),
        };
        assert_eq!(
            pushover_error_message(&rejected),
            "Pushover API returned status code 400: {\"status\":0}"
        );
        let unreachable = omni_alerts::PushoverError {
            status: None,
            body: "timeout".to_owned(),
        };
        assert_eq!(
            pushover_error_message(&unreachable),
            "Pushover request failed: timeout"
        );
    }
}
