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
