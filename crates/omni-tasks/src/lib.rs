//! Scheduling, run history and run log capture.
//!

use std::time::Duration;

use futures::future::BoxFuture;
use omni_store::StoreError;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub mod catch_up;
mod cron;
pub mod health;
pub mod log_capture;
pub mod persistence;
mod registry;
mod scheduler;

pub use cron::{CronSchedule, InvalidScheduleError};
pub use health::{collect_degraded, report_degraded};
pub use log_capture::{RunAttribution, RunLogLayer, RunLogs, current_run};
pub use omni_api::tasks::TaskInfo;
pub use omni_store::LogLine;
pub use persistence::{TaskRunData, TaskRunStatus};
pub use registry::TaskRegistry;
pub use scheduler::Scheduler;

/// Per-task scheduling flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskOptions {
    /// Uniform random delay in `[0, jitter)` before each scheduled run.
    pub jitter: Duration,
    pub run_on_startup: bool,
}

/// What started a run; serialized `"schedule" | "manual" | "startup" | "catchup"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Schedule,
    Manual,
    Startup,
    Catchup,
}

/// Context handed to a running task.
#[derive(Clone, Debug)]
pub struct RunContext {
    pub run_id: String,
    pub task_name: String,
    pub trigger: Trigger,
    /// Cron occurrence a catch-up run recovers.
    pub scheduled_for: Option<i64>,
    /// Shutdown signal; advisory (a started run is never cancelled).
    pub cancel: CancellationToken,
}

/// A scheduled task.
pub trait Task: Send + Sync + 'static {
    /// The load-bearing key (persisted in run history).
    fn name(&self) -> &str;
    fn display_name(&self) -> Option<&str> {
        None
    }
    fn schedule(&self) -> &CronSchedule;
    fn options(&self) -> TaskOptions;
    fn run<'a>(&'a self, cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>>;
    fn accepts_manual_input(&self) -> bool {
        false
    }
    fn run_manual<'a>(
        &'a self,
        cx: &'a RunContext,
        _input: serde_json::Value,
    ) -> BoxFuture<'a, Result<(), TaskError>> {
        self.run(cx)
    }
    /// Optional one-line result of the most recent run.
    fn last_run_summary(&self) -> Option<String> {
        None
    }
}

/// A task failure; `message` is stored as the run's `error`.
#[derive(thiserror::Error, Debug)]
#[error("{message}")]
pub struct TaskError {
    pub message: String,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl TaskError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Wraps an error; the message is the error's own display.
    pub fn from_error(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Task \"{0}\" is already registered")]
pub struct DuplicateTaskError(pub String);

/// `POST /api/tasks/:name/run` and `run_now_and_wait` failures. The first
/// three map to 404 / 409 / 400.
#[derive(Debug, thiserror::Error)]
pub enum RunNowError {
    #[error("Unknown task \"{name}\"")]
    NotFound { name: String },
    #[error("Task \"{name}\" is already running")]
    AlreadyRunning { name: String },
    /// The task takes no manual input.
    #[error("Task \"{name}\" does not accept manual input")]
    ManualInputUnsupported { name: String },
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The awaited run finished as an error (already durably recorded).
    #[error("Task run {run_id} failed: {message}")]
    RunFailed { run_id: String, message: String },
    /// Shutdown began before the queued run could start.
    #[error("Task run was abandoned because the service is shutting down")]
    Shutdown,
}

/// The settled run returned by [`TaskRegistry::run_now_and_wait`].
#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    pub run: TaskRunData,
}

/// Task lifecycle notifications for dashboard SSE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRunEvent {
    pub kind: TaskRunEventKind,
    pub task_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskRunEventKind {
    RunStarted,
    RunFinished,
}

/// Per-line events for open log viewers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunLogEvent {
    Line { run_id: String, line: LogLine },
    End { run_id: String },
}

/// App-level changes that trigger a dashboard snapshot rebroadcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEvent {
    DataDeleted,
    StreamersChanged,
}

/// Process-local pub/sub. Lagging receivers drop old events (observers never
/// block or fail task state).
#[derive(Clone, Debug)]
pub struct EventBus {
    task_runs: broadcast::Sender<TaskRunEvent>,
    run_logs: broadcast::Sender<RunLogEvent>,
    app: broadcast::Sender<AppEvent>,
}

impl EventBus {
    /// `capacity` is per channel.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            task_runs: broadcast::channel(capacity).0,
            run_logs: broadcast::channel(capacity).0,
            app: broadcast::channel(capacity).0,
        }
    }

    pub fn task_runs(&self) -> broadcast::Receiver<TaskRunEvent> {
        self.task_runs.subscribe()
    }

    pub fn run_logs(&self) -> broadcast::Receiver<RunLogEvent> {
        self.run_logs.subscribe()
    }

    pub fn app(&self) -> broadcast::Receiver<AppEvent> {
        self.app.subscribe()
    }

    /// No receivers is not an error.
    pub fn emit_app(&self, e: AppEvent) {
        let _ = self.app.send(e);
    }

    pub fn emit_task_run(&self, e: TaskRunEvent) {
        let _ = self.task_runs.send(e);
    }

    pub fn emit_run_log(&self, e: RunLogEvent) {
        let _ = self.run_logs.send(e);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bus_delivers_to_subscribers() {
        let bus = EventBus::new(8);
        let mut app = bus.app();
        bus.emit_app(AppEvent::StreamersChanged);
        assert_eq!(app.recv().await.ok(), Some(AppEvent::StreamersChanged));
    }

    #[test]
    fn trigger_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Trigger::Catchup).ok().as_deref(),
            Some("\"catchup\"")
        );
    }
}
