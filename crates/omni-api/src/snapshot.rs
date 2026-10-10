//! Dashboard snapshot: `GET /api/snapshot` and every `snapshot` frame of
//! the `/api/events` SSE stream.

use serde::{Deserialize, Serialize};

use crate::build::BuildIdentity;
use crate::media::OnDeckItem;
use crate::runs::Run;
use crate::streamers::StreamerView;
use crate::tasks::TaskInfo;

/// Recent runs carried by a snapshot (`SNAPSHOT_RUN_LIMIT`).
pub const SNAPSHOT_RUN_LIMIT: usize = 30;

/// The full dashboard state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub tasks: Vec<TaskInfo>,
    /// Live streamers first (display order), then offline ones.
    pub streamers: Vec<StreamerView>,
    /// Newest first, at most [`SNAPSHOT_RUN_LIMIT`].
    pub runs: Vec<Run>,
    pub on_deck: Vec<OnDeckItem>,
    /// Constant per server process; a change means a deploy replaced it.
    pub build: BuildIdentity,
}

/// SSE event names on `/api/events`.
pub mod events {
    /// A full [`super::Snapshot`] as JSON; ids increase monotonically per process.
    pub const SNAPSHOT: &str = "snapshot";
    /// Keep-alive every 25 s; data is the server's epoch ms, no id.
    pub const PING: &str = "ping";
}

/// Paths of the dashboard routes.
pub mod paths {
    pub const SNAPSHOT: &str = "/api/snapshot";
    pub const EVENTS: &str = "/api/events";
}
