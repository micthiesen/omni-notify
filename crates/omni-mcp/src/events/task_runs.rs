//! `task.run_finished` publisher.
//!
//! While a subscription is active, `TaskRunEvents` scans run history every
//! 30 seconds for runs that finished in the last ten minutes and publishes the
//! ones some subscription wants. Outbox receipts make rescans idempotent, so
//! no cursor is stored; the overlapping window also covers a run whose end is
//! written just after a later run's. Runs that finished before the window
//! (while nobody was subscribed, or long before a restart) are history, not
//! news. Runs no subscription matches are never published, so the frequent
//! successful runs leave no receipts behind.

use omni_api::events::{TaskRunFinished, TaskRunOutcome};
use omni_core::clock::SharedClock;
use omni_store::entity::EntityOps as _;
use omni_store::{Store, StoreError};
use omni_tasks::TaskRunData;
use serde_json::{Map, Value};

use super::catalog::{TASK_RUN_FINISHED, event_definition};
use super::publisher::{event_key, receipt_key};
use super::service::{McpEventService, PublishInput};

/// How far back each pass looks; well beyond the 30-second poll.
pub const LOOKBACK_MS: i64 = 10 * 60_000;

/// The payload for a finished run; `None` while it is still running.
pub fn run_payload(run: &TaskRunData) -> Option<Map<String, Value>> {
    let finished_at = run.finished_at?;
    // Decoded through the stored spelling so every settled status maps by
    // name; a running row (or an unknown status) has no outcome.
    let status: TaskRunOutcome = serde_json::to_value(run.status)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())?;
    let payload = TaskRunFinished {
        run_id: run.run_id.clone(),
        task_name: run.task_name.clone(),
        trigger: omni_api::runs::Run::from(run).trigger,
        status,
        started_at: omni_core::js::to_iso_string(run.started_at),
        finished_at: omni_core::js::to_iso_string(finished_at),
    };
    match serde_json::to_value(payload) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// Publishes `task.run_finished`; cheap to clone.
#[derive(Clone)]
pub struct TaskRunWatcher {
    events: McpEventService,
    store: Store,
    clock: SharedClock,
}

impl TaskRunWatcher {
    pub fn new(events: McpEventService, store: Store, clock: SharedClock) -> Self {
        Self {
            events,
            store,
            clock,
        }
    }

    /// One pass; returns how many runs were queued for delivery.
    pub async fn poll(&self) -> Result<usize, StoreError> {
        let Some(definition) = event_definition(TASK_RUN_FINISHED) else {
            return Ok(0);
        };
        let subscriptions = self.events.active_arguments(TASK_RUN_FINISHED).await?;
        if subscriptions.is_empty() {
            return Ok(0);
        }
        let since = self.clock.now_ms() - LOOKBACK_MS;
        let mut runs: Vec<TaskRunData> = self
            .store
            .read(|docs| docs.get_all::<TaskRunData>())
            .await?
            .into_iter()
            .filter(|run| run.finished_at.is_some_and(|at| at >= since))
            .collect();
        runs.sort_by_key(|run| run.finished_at);
        let mut queued = 0;
        for run in runs {
            let Some(data) = run_payload(&run) else {
                continue;
            };
            if !subscriptions
                .iter()
                .any(|args| definition.matches(args, &data))
            {
                continue;
            }
            let published = self
                .events
                .publish(PublishInput {
                    name: TASK_RUN_FINISHED.to_owned(),
                    receipt_key: receipt_key(TASK_RUN_FINISHED, &run.run_id),
                    event_key: event_key(TASK_RUN_FINISHED, &run.run_id),
                    timestamp: omni_core::js::to_iso_string(run.finished_at.unwrap_or_default()),
                    data,
                })
                .await?;
            if published {
                queued += 1;
            }
        }
        Ok(queued)
    }
}
