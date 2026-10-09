//! Aggregate streamer state (entity `streamer-status`).
//!
//! Dates are JS `Date`s (CBOR tag 1); older rows may carry explicit
//! `undefined` fields, which decode as absent.

use omni_store::Store;
use omni_store::StoreError;
use omni_store::cbor::{Extra, JsDate};
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use serde::{Deserialize, Serialize};

use crate::error::LiveError;
use crate::platform::{Platform, PlatformBinding};

/// One per-platform observation behind `viewerCount`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSource {
    pub platform: Platform,
    pub username: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveStatus {
    pub streamer_id: String,
    pub primary: PlatformBinding,
    pub primary_title: String,
    /// Epoch ms.
    pub started_at: i64,
    pub max_viewer_count: i64,
    /// Current summed viewer count (absent on old rows).
    pub viewer_count: Option<i64>,
    /// Absent on old rows.
    pub sources: Option<Vec<LiveSource>>,
    pub category: Option<String>,
    pub extra: Extra,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfflineStatus {
    pub streamer_id: String,
    pub last_ended_at: Option<i64>,
    pub last_started_at: Option<i64>,
    pub last_max_viewer_count: Option<i64>,
    pub extra: Extra,
}

impl OfflineStatus {
    /// The default for a streamer never seen live.
    pub fn never_live(streamer_id: impl Into<String>) -> Self {
        Self {
            streamer_id: streamer_id.into(),
            last_ended_at: None,
            last_started_at: None,
            last_max_viewer_count: None,
            extra: Extra::new(),
        }
    }
}

/// `StreamerStatus`, discriminated by `isLive`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StreamerStatusDoc", into = "StreamerStatusDoc")]
pub enum StreamerStatus {
    Live(LiveStatus),
    Offline(OfflineStatus),
}

impl StreamerStatus {
    pub fn streamer_id(&self) -> &str {
        match self {
            StreamerStatus::Live(live) => &live.streamer_id,
            StreamerStatus::Offline(offline) => &offline.streamer_id,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, StreamerStatus::Live(_))
    }

    pub fn as_live(&self) -> Option<&LiveStatus> {
        match self {
            StreamerStatus::Live(live) => Some(live),
            StreamerStatus::Offline(_) => None,
        }
    }
}

/// The persisted shape; field order matches the stored rows of both variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamerStatusDoc {
    streamer_id: String,
    is_live: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    primary: Option<PlatformBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    primary_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at: Option<JsDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_viewer_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    viewer_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sources: Option<Vec<LiveSource>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_ended_at: Option<JsDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_started_at: Option<JsDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_max_viewer_count: Option<i64>,
    #[serde(flatten)]
    extra: Extra,
}

impl TryFrom<StreamerStatusDoc> for StreamerStatus {
    type Error = String;

    fn try_from(doc: StreamerStatusDoc) -> Result<Self, String> {
        if !doc.is_live {
            return Ok(StreamerStatus::Offline(OfflineStatus {
                streamer_id: doc.streamer_id,
                last_ended_at: doc.last_ended_at.map(|d| d.0),
                last_started_at: doc.last_started_at.map(|d| d.0),
                last_max_viewer_count: doc.last_max_viewer_count,
                extra: doc.extra,
            }));
        }
        let missing = |field: &str| format!("live status is missing {field}");
        Ok(StreamerStatus::Live(LiveStatus {
            primary: doc.primary.ok_or_else(|| missing("primary"))?,
            primary_title: doc.primary_title.ok_or_else(|| missing("primaryTitle"))?,
            started_at: doc.started_at.ok_or_else(|| missing("startedAt"))?.0,
            max_viewer_count: doc
                .max_viewer_count
                .ok_or_else(|| missing("maxViewerCount"))?,
            streamer_id: doc.streamer_id,
            viewer_count: doc.viewer_count,
            sources: doc.sources,
            category: doc.category,
            extra: doc.extra,
        }))
    }
}

impl From<StreamerStatus> for StreamerStatusDoc {
    fn from(status: StreamerStatus) -> Self {
        match status {
            StreamerStatus::Live(live) => StreamerStatusDoc {
                streamer_id: live.streamer_id,
                is_live: true,
                primary: Some(live.primary),
                primary_title: Some(live.primary_title),
                started_at: Some(JsDate(live.started_at)),
                max_viewer_count: Some(live.max_viewer_count),
                viewer_count: live.viewer_count,
                sources: live.sources,
                category: live.category,
                last_ended_at: None,
                last_started_at: None,
                last_max_viewer_count: None,
                extra: live.extra,
            },
            StreamerStatus::Offline(offline) => StreamerStatusDoc {
                streamer_id: offline.streamer_id,
                is_live: false,
                primary: None,
                primary_title: None,
                started_at: None,
                max_viewer_count: None,
                viewer_count: None,
                sources: None,
                category: None,
                last_ended_at: offline.last_ended_at.map(JsDate),
                last_started_at: offline.last_started_at.map(JsDate),
                last_max_viewer_count: offline.last_max_viewer_count,
                extra: offline.extra,
            },
        }
    }
}

impl Entity for StreamerStatus {
    const NAME: &'static str = "streamer-status";
    type Key = String;

    fn key(&self) -> String {
        self.streamer_id().to_owned()
    }
}

/// The stored status, or offline-never-live.
pub async fn get_status(store: &Store, streamer_id: &str) -> Result<StreamerStatus, LiveError> {
    let id = streamer_id.to_owned();
    let found = store
        .read(move |docs| docs.get::<StreamerStatus>(&id))
        .await
        .map_err(LiveError::persistence("read streamer status"))?;
    Ok(found.unwrap_or_else(|| StreamerStatus::Offline(OfflineStatus::never_live(streamer_id))))
}

pub async fn upsert_status(store: &Store, status: StreamerStatus) -> Result<(), LiveError> {
    store
        .write(move |tx| tx.upsert(&status, UpsertOpts::default()))
        .await
        .map_err(|e: StoreError| LiveError::persistence("upsert streamer status")(e))
}
