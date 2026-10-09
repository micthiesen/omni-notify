//! Scheduled tasks: `McpEventDelivery` (30 s outbox sweep),
//! `ClaudeSessionEvents` (15 s host poll while subscribed) and
//! `TaskRunEvents` (30 s run history scan while subscribed).

use std::time::Duration;

use futures::future::BoxFuture;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::events::claude_sessions::ClaudeSessionWatcher;
use crate::events::service::McpEventService;
use crate::events::task_runs::TaskRunWatcher;

pub const MCP_EVENT_DELIVERY_SCHEDULE: &str = "*/30 * * * * *";
pub const CLAUDE_SESSION_EVENTS_SCHEDULE: &str = "*/15 * * * * *";
pub const TASK_RUN_EVENTS_SCHEDULE: &str = "15,45 * * * * *";

fn options() -> TaskOptions {
    TaskOptions {
        jitter: Duration::ZERO,
        run_on_startup: false,
    }
}

/// Retries due outbox rows without changing event IDs.
pub struct McpEventDeliveryTask {
    events: McpEventService,
    schedule: CronSchedule,
}

impl McpEventDeliveryTask {
    pub fn new(events: McpEventService, schedule: CronSchedule) -> Self {
        Self { events, schedule }
    }
}

impl Task for McpEventDeliveryTask {
    fn name(&self) -> &str {
        "McpEventDelivery"
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        options()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            self.events
                .drain()
                .await
                .map(|_| ())
                .map_err(TaskError::from_error)
        })
    }
}

/// Publishes `claude.session.turn_finished` while a subscription is active.
pub struct ClaudeSessionEventsTask {
    watcher: ClaudeSessionWatcher,
    schedule: CronSchedule,
}

impl ClaudeSessionEventsTask {
    pub fn new(watcher: ClaudeSessionWatcher, schedule: CronSchedule) -> Self {
        Self { watcher, schedule }
    }
}

impl Task for ClaudeSessionEventsTask {
    fn name(&self) -> &str {
        "ClaudeSessionEvents"
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        options()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move { self.watcher.poll().await.map_err(TaskError::from_error) })
    }
}

/// Publishes `task.run_finished` while a subscription is active.
pub struct TaskRunEventsTask {
    watcher: TaskRunWatcher,
    schedule: CronSchedule,
}

impl TaskRunEventsTask {
    pub fn new(watcher: TaskRunWatcher, schedule: CronSchedule) -> Self {
        Self { watcher, schedule }
    }
}

impl Task for TaskRunEventsTask {
    fn name(&self) -> &str {
        "TaskRunEvents"
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        options()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            self.watcher
                .poll()
                .await
                .map(|_| ())
                .map_err(TaskError::from_error)
        })
    }
}
