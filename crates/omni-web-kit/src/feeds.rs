//! Shared, lazily refreshed feeds the shell and several pages read:
//! the workspace overview (rail badge, palette Research group, Home).
//! It refetches on the `workspace-updated` window event and when a
//! workspace task finishes a run in the snapshot.

use leptos::prelude::*;
use omni_api::workspaces::WorkspaceOverview;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

use crate::api;
use crate::live::use_live_data;
use crate::task::{on_cleanup_local, spawn_scoped};

#[derive(Clone, Copy)]
pub struct WorkspaceFeed {
    pub workspaces: RwSignal<Option<Vec<WorkspaceOverview>>>,
    pub error: RwSignal<Option<String>>,
    reload: RwSignal<u32>,
}

impl WorkspaceFeed {
    pub fn refresh(self) {
        self.reload.update(|n| *n += 1);
    }

    /// Pending actions across all workspaces.
    pub fn pending_actions(self) -> u64 {
        self.workspaces.with(|w| {
            w.as_ref()
                .map_or(0, |w| w.iter().map(|o| o.pending_action_count).sum())
        })
    }
}

/// Starts the feed under the current owner and provides it.
pub fn provide_workspace_feed() -> WorkspaceFeed {
    let feed = WorkspaceFeed {
        workspaces: RwSignal::new(None),
        error: RwSignal::new(None),
        reload: RwSignal::new(0),
    };
    provide_context(feed);
    let live = use_live_data();
    // The finish time of the newest workspace-task run.
    let finished = Memo::new(move |_| {
        let names: Vec<String> = feed.workspaces.with(|w| {
            w.as_ref()
                .map(|w| w.iter().map(|o| o.definition.task_name.clone()).collect())
                .unwrap_or_default()
        });
        live.snapshot.with(|s| {
            s.as_ref().and_then(|s| {
                s.tasks
                    .iter()
                    .filter(|t| names.contains(&t.name))
                    .filter_map(|t| t.last_run.as_ref().and_then(|r| r.finished_at))
                    .max()
            })
        })
    });
    Effect::new(move |_| {
        feed.reload.track();
        finished.track();
        spawn_scoped(async move {
            match api::fetch_workspaces().await {
                Ok(res) => {
                    feed.workspaces.set(Some(res.workspaces));
                    feed.error.set(None);
                }
                Err(e) => feed.error.set(Some(e.message().to_owned())),
            }
        });
    });
    let on_updated = Closure::<dyn FnMut()>::new(move || feed.refresh());
    let win = window();
    let _ = win
        .add_event_listener_with_callback("workspace-updated", on_updated.as_ref().unchecked_ref());
    on_cleanup_local(move || {
        let _ = win.remove_event_listener_with_callback(
            "workspace-updated",
            on_updated.as_ref().unchecked_ref(),
        );
    });
    feed
}

/// The feed, or an empty one outside the shell.
pub fn use_workspace_feed() -> WorkspaceFeed {
    use_context::<WorkspaceFeed>().unwrap_or_else(|| WorkspaceFeed {
        workspaces: RwSignal::new(None),
        error: RwSignal::new(None),
        reload: RwSignal::new(0),
    })
}

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
