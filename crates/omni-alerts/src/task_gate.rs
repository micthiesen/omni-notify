//! Task failure alert gate: a task's run-failure ERROR stays visible as a
//! failed run and log line, but reaches Pushover only once the task's leading
//! streak of bad runs satisfies a [`PersistenceRule`] in durable run history,
//! so a restart cannot reset the streak and a single network blip in a
//! frequent poller never notifies. A success ends the incident.

use futures::future::BoxFuture;
use omni_store::Store;
use omni_tasks::health::{PersistenceRule, failure_alert_titles};
use omni_tasks::persistence::{self, KEEP_PER_TASK};

use crate::AlertGate;

/// [`AlertGate`] over the durable `task-run` history of a fixed set of tasks.
pub struct TaskFailureGate {
    store: Store,
    rule: PersistenceRule,
    /// `(alert title, task name)` for every gated title.
    titles: Vec<(String, String)>,
}

impl TaskFailureGate {
    pub fn new(store: Store, tasks: &[&str], rule: PersistenceRule) -> Self {
        let titles = tasks
            .iter()
            .flat_map(|task| {
                failure_alert_titles(task)
                    .into_iter()
                    .map(move |title| (title, (*task).to_owned()))
            })
            .collect();
        Self {
            store,
            rule,
            titles,
        }
    }

    fn task_for(&self, title: &str) -> Option<&str> {
        self.titles
            .iter()
            .find(|(gated, _)| gated == title)
            .map(|(_, task)| task.as_str())
    }
}

impl AlertGate for TaskFailureGate {
    fn applies(&self, title: &str) -> bool {
        self.task_for(title).is_some()
    }

    fn should_notify<'a>(&'a self, title: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let Some(task) = self.task_for(title) else {
                return true;
            };
            // Missing history cannot establish a persistent failure.
            persistence::get_runs(&self.store, Some(task), KEEP_PER_TASK)
                .await
                .map(|runs| self.rule.is_persistent(&runs))
                .unwrap_or(false)
        })
    }
}
