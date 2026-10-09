//! Completed live sessions (`sessions.ts`, entity `streamer-sessions`).

use omni_store::Store;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use serde::{Deserialize, Serialize};

use crate::error::LiveError;
use crate::platform::Platform;
use crate::status::LiveStatus;

/// Sessions kept per streamer.
pub const MAX_SESSIONS: usize = 300;
/// 180 days.
pub const MAX_SESSION_AGE_MS: i64 = 180 * 24 * 60 * 60 * 1000;

/// One completed live session, recorded when the streamer goes offline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSession {
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    /// Peak of the summed viewer count across live bindings.
    pub peak_viewers: i64,
    /// Primary binding's title/platform when the session closed.
    pub title: String,
    pub platform: Platform,
    pub username: String,
}

/// `StreamSessionsData`: oldest to newest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSessions {
    pub streamer_id: String,
    pub sessions: Vec<StreamSession>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl StreamSessions {
    pub fn empty(streamer_id: impl Into<String>) -> Self {
        Self {
            streamer_id: streamer_id.into(),
            sessions: Vec::new(),
            extra: Extra::new(),
        }
    }
}

impl Entity for StreamSessions {
    const NAME: &'static str = "streamer-sessions";
    type Key = String;

    fn key(&self) -> String {
        self.streamer_id.clone()
    }
}

/// Pure: append, then prune by age and count.
pub fn append_session(mut data: StreamSessions, session: StreamSession) -> StreamSessions {
    let cutoff = session.ended_at - MAX_SESSION_AGE_MS;
    data.sessions.push(session);
    data.sessions.retain(|s| s.ended_at >= cutoff);
    let excess = data.sessions.len().saturating_sub(MAX_SESSIONS);
    data.sessions.drain(..excess);
    data
}

/// Pure: the session closing out a live status.
pub fn session_from_live_status(live: &LiveStatus, ended_at: i64) -> StreamSession {
    StreamSession {
        started_at: live.started_at,
        ended_at,
        duration_ms: (ended_at - live.started_at).max(0),
        peak_viewers: live.max_viewer_count,
        title: live.primary_title.clone(),
        platform: live.primary.platform,
        username: live.primary.username.clone(),
    }
}

pub async fn get_sessions(store: &Store, streamer_id: &str) -> Result<StreamSessions, LiveError> {
    let id = streamer_id.to_owned();
    let found = store
        .read(move |docs| docs.get::<StreamSessions>(&id))
        .await
        .map_err(LiveError::persistence("read stream sessions"))?;
    Ok(found.unwrap_or_else(|| StreamSessions::empty(streamer_id)))
}

/// Appends the completed session in one transaction.
pub async fn record_completed_session(
    store: &Store,
    live: &LiveStatus,
    ended_at: i64,
) -> Result<(), LiveError> {
    let id = live.streamer_id.clone();
    let session = session_from_live_status(live, ended_at);
    store
        .write(move |tx| {
            let data = tx
                .get::<StreamSessions>(&id)?
                .unwrap_or_else(|| StreamSessions::empty(id.clone()));
            tx.upsert(&append_session(data, session), UpsertOpts::default())
        })
        .await
        .map_err(LiveError::persistence("record completed stream session"))
}
