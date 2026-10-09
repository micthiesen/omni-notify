//! Castro failure alert gate (`src/alerts/castro.ts`): cleanup failures stay
//! visible as failed runs and ERROR logs, but reach Pushover only after at
//! least three consecutive failed runs spanning twelve hours (minus five
//! minutes of schedule jitter), judged from durable run history so a restart
//! cannot reset the streak. A success ends the incident.
//!
//! Deviation: the client's per-episode `"Castro inbox clear failed"` ERROR is
//! gated too. TS let it reach Pushover ungated, so one isolated HTTP 500 or
//! socket failure during cleanup notified, contrary to the AGENTS.md
//! invariant. It is logged while the run is still `running`, so the gate
//! withholds it and the run's own failure alert carries the incident.

use futures::future::BoxFuture;
use omni_alerts::AlertGate;
use omni_store::Store;
use omni_tasks::persistence::{self, KEEP_PER_TASK};
use omni_tasks::{TaskRunData, TaskRunStatus};

use super::cleanup::TASK_NAME;

const MIN_FAILURE_SPAN_MS: i64 = 12 * 60 * 60_000;
/// Cron jitter around the twelve-hour boundary.
const JITTER_MS: i64 = 5 * 60_000;
const MIN_CONSECUTIVE_FAILURES: usize = 3;

/// History is newest first; the leading run of `error` statuses is the streak.
pub fn has_persistent_castro_failure(runs: &[TaskRunData]) -> bool {
    let failures: Vec<&TaskRunData> = runs
        .iter()
        .take_while(|run| run.status == TaskRunStatus::Error)
        .collect();
    match (failures.first(), failures.last()) {
        (Some(newest), Some(oldest)) if failures.len() >= MIN_CONSECUTIVE_FAILURES => {
            newest.started_at - oldest.started_at >= MIN_FAILURE_SPAN_MS - JITTER_MS
        }
        _ => false,
    }
}

/// The client error logged when clearing one Inbox episode fails.
pub const INBOX_CLEAR_FAILED_TITLE: &str = "Castro inbox clear failed";

/// The alert titles the task registry and scheduler log for this task, plus
/// the client's inbox-clear failure.
pub fn gated_titles() -> [String; 4] {
    [
        format!("Error running task \"{TASK_NAME}\""),
        format!("Manual run of \"{TASK_NAME}\" failed"),
        format!("Catch-up run of \"{TASK_NAME}\" failed"),
        INBOX_CLEAR_FAILED_TITLE.to_owned(),
    ]
}

/// [`AlertGate`] over the durable `task-run` history.
pub struct CastroFailureGate {
    store: Store,
    titles: [String; 4],
}

impl CastroFailureGate {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            titles: gated_titles(),
        }
    }
}

impl AlertGate for CastroFailureGate {
    fn applies(&self, title: &str) -> bool {
        self.titles.iter().any(|t| t == title)
    }

    fn should_notify<'a>(&'a self, _title: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // Missing history cannot establish a persistent Castro failure.
            persistence::get_runs(&self.store, Some(TASK_NAME), KEEP_PER_TASK)
                .await
                .map(|runs| has_persistent_castro_failure(&runs))
                .unwrap_or(false)
        })
    }
}
