//! Viewer metric entities.

use jiff::tz::TimeZone;
use omni_store::Store;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use serde::{Deserialize, Serialize};

use super::windows::{DailyBucket, prune_buckets, update_daily_bucket};
use crate::error::LiveError;
use crate::identity::canonical_binding_key;
use crate::platform::Platform;

const PLATFORM_METRICS_RETENTION_DAYS: i64 = 100;

/// `streamer-viewer-metrics`: the summed count's daily peaks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerMetrics {
    pub streamer_id: String,
    pub daily_buckets: Vec<DailyBucket>,
    pub all_time_max: i64,
    pub all_time_max_timestamp: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl ViewerMetrics {
    pub fn empty(streamer_id: impl Into<String>) -> Self {
        Self {
            streamer_id: streamer_id.into(),
            daily_buckets: Vec::new(),
            all_time_max: 0,
            all_time_max_timestamp: 0,
            extra: Extra::new(),
        }
    }
}

impl Entity for ViewerMetrics {
    const NAME: &'static str = "streamer-viewer-metrics";
    type Key = String;

    fn key(&self) -> String {
        self.streamer_id.clone()
    }
}

/// `streamer-platform-viewer-metrics`: one series per platform account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformViewerMetrics {
    pub streamer_id: String,
    pub platform: String,
    pub username: String,
    pub daily_buckets: Vec<DailyBucket>,
    pub all_time_max: i64,
    pub all_time_max_timestamp: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PlatformViewerMetrics {
    const NAME: &'static str = "streamer-platform-viewer-metrics";
    type Key = (String, String, String);

    fn key(&self) -> (String, String, String) {
        (
            self.streamer_id.clone(),
            self.platform.clone(),
            self.username.clone(),
        )
    }
}

pub async fn get_viewer_metrics(
    store: &Store,
    streamer_id: &str,
) -> Result<ViewerMetrics, LiveError> {
    let id = streamer_id.to_owned();
    let found = store
        .read(move |docs| docs.get::<ViewerMetrics>(&id))
        .await
        .map_err(LiveError::persistence("read viewer metrics"))?;
    Ok(found.unwrap_or_else(|| ViewerMetrics::empty(streamer_id)))
}

/// Every platform series of a streamer.
pub async fn get_platform_viewer_metrics(
    store: &Store,
    streamer_id: &str,
) -> Result<Vec<PlatformViewerMetrics>, LiveError> {
    let id = streamer_id.to_owned();
    store
        .read(move |docs| {
            Ok(docs
                .get_all::<PlatformViewerMetrics>()?
                .into_iter()
                .filter(|m| m.streamer_id == id)
                .collect())
        })
        .await
        .map_err(LiveError::persistence("list platform viewer metrics"))
}

/// Records one platform observation (canonical username, 100-day retention).
pub async fn record_platform_viewer_count(
    store: &Store,
    streamer_id: &str,
    platform: Platform,
    username: &str,
    viewer_count: i64,
    now: i64,
    tz: &TimeZone,
) -> Result<(), LiveError> {
    let canonical = canonical_binding_key(platform, username);
    let username = canonical
        .split_once(':')
        .map_or(canonical.as_str(), |(_, rest)| rest)
        .to_owned();
    let key = (
        streamer_id.to_owned(),
        platform.as_str().to_owned(),
        username,
    );
    let tz = tz.clone();
    store
        .write(move |tx| {
            let mut metrics =
                tx.get::<PlatformViewerMetrics>(&key)?
                    .unwrap_or_else(|| PlatformViewerMetrics {
                        streamer_id: key.0.clone(),
                        platform: key.1.clone(),
                        username: key.2.clone(),
                        daily_buckets: Vec::new(),
                        all_time_max: 0,
                        all_time_max_timestamp: 0,
                        extra: Extra::new(),
                    });
            metrics.daily_buckets = update_daily_bucket(&metrics.daily_buckets, viewer_count, now);
            metrics.daily_buckets = prune_buckets(
                &metrics.daily_buckets,
                PLATFORM_METRICS_RETENTION_DAYS,
                now,
                &tz,
            );
            if viewer_count > metrics.all_time_max {
                metrics.all_time_max = viewer_count;
                metrics.all_time_max_timestamp = now;
            }
            tx.upsert(&metrics, UpsertOpts::default())
        })
        .await
        .map_err(LiveError::persistence("record platform viewer count"))
}
