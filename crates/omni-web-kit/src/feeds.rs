//! Snapshot-driven reload helpers: the newest run of a task, and data
//! reloaded whenever that task finishes a run.

use leptos::prelude::*;

use crate::api;
use crate::live::use_live_data;
use crate::task::spawn_scoped;

/// The newest finished run of `task` in the snapshot (changes whenever the
/// task completes a run, so dependent views refetch live).
pub fn use_task_finished(task: &'static str) -> Memo<Option<i64>> {
    let live = use_live_data();
    Memo::new(move |_| {
        live.snapshot.with(|s| {
            s.as_ref().and_then(|s| {
                s.tasks
                    .iter()
                    .find(|t| t.name == task)?
                    .last_run
                    .as_ref()?
                    .finished_at
            })
        })
    })
}

/// Data loaded on mount and reloaded whenever `task` finishes a run.
/// Previous data stays while a reload is in flight (`refreshing`).
pub struct TaskBacked<T: Send + Sync + 'static> {
    pub data: RwSignal<Option<T>>,
    pub error: RwSignal<Option<String>>,
    pub refreshing: RwSignal<bool>,
    reload: RwSignal<u32>,
}

impl<T: Send + Sync + 'static> Clone for TaskBacked<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for TaskBacked<T> {}

impl<T: Send + Sync + 'static> TaskBacked<T> {
    pub fn reload(self) {
        self.reload.update(|n| *n += 1);
    }
}

pub fn use_task_backed<T, Fut, L>(task: &'static str, load: L) -> TaskBacked<T>
where
    T: Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<T, api::ApiClientError>> + 'static,
    L: Fn() -> Fut + 'static,
{
    let finished = use_task_finished(task);
    let backed = TaskBacked {
        data: RwSignal::new(None),
        error: RwSignal::new(None),
        refreshing: RwSignal::new(false),
        reload: RwSignal::new(0),
    };
    let load = std::rc::Rc::new(load);
    Effect::new(move |_| {
        finished.track();
        backed.reload.track();
        let load = load.clone();
        backed.refreshing.set(true);
        spawn_scoped(async move {
            match load().await {
                Ok(value) => {
                    backed.data.set(Some(value));
                    backed.error.set(None);
                }
                Err(e) => backed.error.set(Some(e.message().to_owned())),
            }
            backed.refreshing.set(false);
        });
    });
    backed
}
