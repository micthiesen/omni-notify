//! Dashboard state for the whole app.
//!
//! One `EventSource("/api/events")` per visible tab: every `snapshot` frame
//! replaces the snapshot. `/api/snapshot` is polled immediately (first paint)
//! and every 10 s, skipped while the stream is live or the tab is hidden. On a
//! stream `error` the connection reads "polling"; a CLOSED stream reconnects
//! after 5 s.
//!
//! The stream opens only after the document's `load` and closes while the tab
//! is hidden: browsers allow six HTTP/1.1 connections per origin, and a held
//! stream per background tab otherwise starves page loads and API fetches.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use futures::future::{Either, select};
use leptos::prelude::*;

use crate::api::{self, ApiClientError, Snapshot};
use crate::components::streamers::{live_streamers, viewer_number};
use crate::sse::{SseConnection, SseMessage};
use crate::task::{sleep, spawn_scoped};
use crate::utils::js::now_ms;

/// Viewer samples kept per live streamer (4 h at the 20 s live-check cadence).
pub const VIEWER_HISTORY_LIMIT: usize = 720;

pub const POLL_INTERVAL: Duration = Duration::from_secs(10);
pub const RECONNECT_DELAY: Duration = Duration::from_secs(5);
/// How often a paused stream rechecks page visibility.
const VISIBILITY_CHECK: Duration = Duration::from_millis(500);

/// Whether the document has loaded and the tab is visible.
fn page_active() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_none_or(|d| !d.hidden() && d.ready_state() == "complete")
}

fn page_hidden() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_some_and(|d| d.hidden())
}

/// Resolves once the document has loaded and the tab is visible.
async fn until_active() {
    while !page_active() {
        sleep(VISIBILITY_CHECK).await;
    }
}

/// Resolves once the tab is hidden.
async fn until_hidden() {
    while !page_hidden() {
        sleep(VISIBILITY_CHECK).await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Live,
    Polling,
}

impl ConnectionState {
    /// CSS suffix (`conn-<state>`).
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectionState::Connecting => "connecting",
            ConnectionState::Live => "live",
            ConnectionState::Polling => "polling",
        }
    }
}

/// Outcome of a manual task run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    pub ok: bool,
    pub message: String,
}

/// The `LiveData` context.
#[derive(Clone, Copy)]
pub struct LiveData {
    pub snapshot: RwSignal<Option<Snapshot>>,
    pub connection: RwSignal<ConnectionState>,
    pub error: RwSignal<Option<String>>,
    /// Epoch ms of the last snapshot received (0 before the first).
    pub updated_at: RwSignal<f64>,
    /// Per live streamer: `(epoch ms, viewers)` samples observed in this tab,
    /// oldest first. Cleared when the streamer goes offline.
    pub viewer_history: RwSignal<HashMap<String, Vec<(f64, f64)>>>,
}

/// Appends one sample per live streamer and drops offline ones.
pub fn record_viewers(
    history: &mut HashMap<String, Vec<(f64, f64)>>,
    snapshot: &Snapshot,
    now: f64,
) {
    let live = live_streamers(&snapshot.streamers);
    history.retain(|id, _| live.iter().any(|s| &s.id == id));
    for streamer in live {
        let samples = history.entry(streamer.id.clone()).or_default();
        let value = viewer_number(&streamer) as f64;
        if samples
            .last()
            .is_some_and(|(t, v)| *v == value && now - t < 1000.0)
        {
            continue;
        }
        samples.push((now, value));
        let overflow = samples.len().saturating_sub(VIEWER_HISTORY_LIMIT);
        samples.drain(..overflow);
    }
}

impl LiveData {
    /// Replaces the snapshot and records its time and viewer samples.
    pub fn apply(self, next: Snapshot) {
        let now = now_ms();
        self.viewer_history
            .update(|history| record_viewers(history, &next, now));
        self.updated_at.set(now);
        self.snapshot.set(Some(next));
    }

    /// Starts `name` (`POST /api/tasks/:name/run`, or the recommendation run
    /// routes with `max_recommendations`). Flips the task's `running` flag at
    /// once; the SSE snapshot lands ~200 ms later.
    pub async fn run_task(self, name: String, max_recommendations: Option<u32>) -> RunResult {
        match api::run_task_request(&name, max_recommendations).await {
            Ok(_) => {
                self.snapshot.update(|snapshot| {
                    if let Some(snapshot) = snapshot {
                        for task in &mut snapshot.tasks {
                            if task.name == name {
                                task.running = true;
                            }
                        }
                    }
                });
                RunResult {
                    ok: true,
                    message: format!("{name} started"),
                }
            }
            Err(ApiClientError::Api { status: 409, .. }) => RunResult {
                ok: false,
                message: format!("{name} is already running"),
            },
            Err(error) => RunResult {
                ok: false,
                message: error.message().to_owned(),
            },
        }
    }
}

/// Decodes one `snapshot` frame.
pub fn decode_snapshot_frame(data: &str) -> Result<Snapshot, String> {
    serde_json::from_str::<Snapshot>(data).map_err(|e| e.to_string())
}

/// Creates the live-data signals, starts the SSE and poll loops under the
/// current owner, and provides the context.
pub fn provide_live_data() -> LiveData {
    let live = LiveData {
        snapshot: RwSignal::new(None),
        connection: RwSignal::new(ConnectionState::Connecting),
        error: RwSignal::new(None),
        updated_at: RwSignal::new(0.0),
        viewer_history: RwSignal::new(HashMap::new()),
    };
    provide_context(live);
    let is_live = Rc::new(Cell::new(false));

    let poll_live = is_live.clone();
    spawn_scoped(async move {
        loop {
            if !poll_live.get() && !page_hidden() {
                match api::fetch_snapshot().await {
                    Ok(next) => {
                        // A stream frame may have landed during the fetch.
                        if !poll_live.get() {
                            live.apply(next);
                            live.error.set(None);
                        }
                    }
                    Err(error) => live.error.set(Some(error.message().to_owned())),
                }
            }
            sleep(POLL_INTERVAL).await;
        }
    });

    spawn_scoped(async move {
        loop {
            until_active().await;
            if let Some(mut connection) = SseConnection::open("/api/events", &["snapshot"]) {
                let mut hidden = Box::pin(until_hidden());
                loop {
                    let message = match select(Box::pin(connection.next()), hidden).await {
                        Either::Left((Some(message), rest)) => {
                            hidden = rest;
                            message
                        }
                        Either::Left((None, _)) => {
                            sleep(RECONNECT_DELAY).await;
                            break;
                        }
                        Either::Right(_) => {
                            // Dropping the connection closes the stream.
                            is_live.set(false);
                            break;
                        }
                    };
                    match message {
                        SseMessage::Event { name, data } if name == "snapshot" => {
                            match decode_snapshot_frame(&data) {
                                Ok(next) => {
                                    is_live.set(true);
                                    live.connection.set(ConnectionState::Live);
                                    live.error.set(None);
                                    live.apply(next);
                                }
                                Err(error) => live.error.set(Some(error)),
                            }
                        }
                        SseMessage::Event { .. } => {}
                        SseMessage::Error { closed } => {
                            is_live.set(false);
                            live.connection.set(ConnectionState::Polling);
                            if closed {
                                sleep(RECONNECT_DELAY).await;
                                break;
                            }
                        }
                    }
                }
            } else {
                sleep(RECONNECT_DELAY).await;
            }
        }
    });
    live
}

/// Provides [`LiveData`] to its children.
#[component]
pub fn LiveDataProvider(children: Children) -> impl IntoView {
    provide_live_data();
    children()
}

/// The `LiveData` context (`useLiveData`).
pub fn use_live_data() -> LiveData {
    match use_context::<LiveData>() {
        Some(live) => live,
        None => LiveData {
            snapshot: RwSignal::new(None),
            connection: RwSignal::new(ConnectionState::Connecting),
            error: RwSignal::new(Some("Live data is unavailable".to_owned())),
            updated_at: RwSignal::new(0.0),
            viewer_history: RwSignal::new(HashMap::new()),
        },
    }
}
