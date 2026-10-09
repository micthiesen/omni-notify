//! Dashboard state for the whole app (`frontend/src/live.tsx`).
//!
//! One `EventSource("/api/events")` per tab: every `snapshot` frame replaces
//! the snapshot. `/api/snapshot` is polled immediately (first paint) and every
//! 10 s, skipped while the stream is live. On a stream `error` the connection
//! reads "polling"; a CLOSED stream reconnects after 5 s.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use leptos::prelude::*;

use crate::api::{self, ApiClientError, Snapshot};
use crate::sse::{SseConnection, SseMessage};
use crate::task::{sleep, spawn_scoped};

pub const POLL_INTERVAL: Duration = Duration::from_secs(10);
pub const RECONNECT_DELAY: Duration = Duration::from_secs(5);

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
}

impl LiveData {
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
    };
    provide_context(live);
    let is_live = Rc::new(Cell::new(false));

    let poll_live = is_live.clone();
    spawn_scoped(async move {
        loop {
            if !poll_live.get() {
                match api::fetch_snapshot().await {
                    Ok(next) => {
                        // A stream frame may have landed during the fetch.
                        if !poll_live.get() {
                            live.snapshot.set(Some(next));
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
            if let Some(mut connection) = SseConnection::open("/api/events", &["snapshot"]) {
                while let Some(message) = connection.next().await {
                    match message {
                        SseMessage::Event { name, data } if name == "snapshot" => {
                            match decode_snapshot_frame(&data) {
                                Ok(next) => {
                                    is_live.set(true);
                                    live.connection.set(ConnectionState::Live);
                                    live.error.set(None);
                                    live.snapshot.set(Some(next));
                                }
                                Err(error) => live.error.set(Some(error)),
                            }
                        }
                        SseMessage::Event { .. } => {}
                        SseMessage::Error { closed } => {
                            is_live.set(false);
                            live.connection.set(ConnectionState::Polling);
                            if closed {
                                break;
                            }
                        }
                    }
                }
            }
            sleep(RECONNECT_DELAY).await;
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
        },
    }
}
