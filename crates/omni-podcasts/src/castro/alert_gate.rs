//! Castro failure alert gate: cleanup failures stay
//! visible as failed runs and ERROR logs, but reach Pushover only after at
//! least three consecutive failed runs spanning twelve hours (minus five
//! minutes of schedule jitter), judged from durable run history so a restart
//! cannot reset the streak. A success ends the incident.
//!
//! The client's per-episode `"Castro inbox clear failed"` ERROR is gated too,
//! so one isolated HTTP 500 or socket failure during cleanup never notifies.
//! It is logged while the run is still `running`, so the gate withholds it and
//! the run's own failure alert carries the incident.

use futures::future::BoxFuture;
use omni_alerts::AlertGate;
use omni_store::Store;
use omni_tasks::TaskRunData;
use omni_tasks::health::PERSISTENT_FAILURE;
use omni_tasks::persistence::{self, KEEP_PER_TASK};

use super::cleanup::TASK_NAME;

/// History is newest first; the leading run of bad statuses is the streak,
/// judged by the shared [`PERSISTENT_FAILURE`] rule (three runs spanning
/// twelve hours, less five minutes of jitter). Cleanup never reports a
/// degraded run, so its streaks are failed runs.
pub fn has_persistent_castro_failure(runs: &[TaskRunData]) -> bool {
    PERSISTENT_FAILURE.is_persistent(runs)
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
