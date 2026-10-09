//! Wire views of streamers (`serializeStreamer`, `serializeStreamersForDisplay`
//! in `server.ts`, and the MCP `serializeStreamer` in `mcp/tools/system.ts`).

use omni_api::streamers::{
    DailyViewerBucket, LiveSourceView, LiveStreamerView, LivestreamSourceSummary,
    LivestreamSummary, OfflineStreamerView, PlatformViewerMetrics as PlatformMetricsView,
    StreamSessionView, StreamerBinding, StreamerMetricsResponse, StreamerStatusView, StreamerView,
};
use omni_store::DocOps;
use omni_store::Store;
use omni_store::cbor::{Extra, JsValue};
use omni_store::entity::{Entity, EntityOps, pk};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::display_order::{LiveRank, sort_live_display, sort_offline_display};
use crate::error::LiveError;
use crate::metrics::{DailyBucket, PlatformViewerMetrics, ViewerMetrics};
use crate::platform::PlatformBinding;
use crate::sessions::StreamSession;
use crate::status::{LiveSource, StreamerStatus};
use crate::streamers::{DiscoverySource, Streamer};

/// Key-only view of WP05's `livestream-intelligence` entity: the dashboard
/// embeds its raw document without decoding it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntelligenceDocKey {
    streamer_id: String,
    #[serde(flatten)]
    extra: Extra,
}

impl Entity for IntelligenceDocKey {
    const NAME: &'static str = "livestream-intelligence";
    type Key = String;

    fn key(&self) -> String {
        self.streamer_id.clone()
    }
}

pub fn binding_view(binding: &PlatformBinding) -> StreamerBinding {
    StreamerBinding {
        platform: binding.platform.as_str().to_owned(),
        username: binding.username.clone(),
        url: binding.watch_url(),
    }
}

fn source_view(source: &LiveSource) -> LiveSourceView {
    LiveSourceView {
        platform: source.platform.as_str().to_owned(),
        username: source.username.clone(),
        title: source.title.clone(),
        viewer_count: source.viewer_count,
        category: source.category.clone(),
    }
}

/// `JSON.stringify` of a decoded document: Dates become ISO strings,
/// `undefined` object members are dropped (array slots become `null`),
/// non-finite numbers `null`, Maps and Sets `{}`.
pub fn js_to_json(value: &JsValue) -> Value {
    match value {
        JsValue::Undefined | JsValue::Null | JsValue::Simple(_) => Value::Null,
        JsValue::Bool(b) => Value::Bool(*b),
        JsValue::Int(n) => i64::try_from(*n)
            .map(Value::from)
            .unwrap_or_else(|_| float_json(*n as f64)),
        JsValue::Float(f) => float_json(*f),
        JsValue::String(s) => Value::String(s.clone()),
        JsValue::Bytes(bytes) => {
            let mut buffer = Map::new();
            buffer.insert("type".into(), Value::String("Buffer".into()));
            buffer.insert(
                "data".into(),
                Value::Array(bytes.iter().map(|b| Value::from(*b)).collect()),
            );
            Value::Object(buffer)
        }
        JsValue::Array(items) => Value::Array(items.iter().map(js_to_json).collect()),
        JsValue::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(_, v)| !matches!(v, JsValue::Undefined))
                .map(|(k, v)| (k.clone(), js_to_json(v)))
                .collect(),
        ),
        JsValue::Map(_) | JsValue::Set(_) => Value::Object(Map::new()),
        JsValue::Date(ms) => {
            if ms.is_finite() {
                #[allow(clippy::cast_possible_truncation)]
                Value::String(omni_core::js::to_iso_string(*ms as i64))
            } else {
                Value::Null
            }
        }
        JsValue::BigInt(n) => Value::String(n.to_string()),
        JsValue::Tagged(_, inner) => js_to_json(inner),
    }
}

fn float_json(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
        #[allow(clippy::cast_possible_truncation)]
        return Value::from(f as i64);
    }
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// One streamer's dashboard view from its status (and, when live, the raw
/// intelligence document).
pub fn streamer_view(
    streamer: &Streamer,
    status: &StreamerStatus,
    intelligence: Option<Value>,
) -> StreamerView {
    let bindings = streamer.bindings.iter().map(binding_view).collect();
    match status {
        StreamerStatus::Live(live) => StreamerView::Live(LiveStreamerView {
            id: streamer.id.clone(),
            display_name: streamer.display_name.clone(),
            bindings,
            tier: streamer.tier,
            dgg: streamer.dgg,
            live: Default::default(),
            title: live.primary_title.clone(),
            started_at: live.started_at,
            max_viewer_count: live.max_viewer_count,
            viewer_count: live.viewer_count,
            sources: live.sources.iter().flatten().map(source_view).collect(),
            category: live.category.clone(),
            primary: binding_view(&live.primary),
            intelligence,
        }),
        StreamerStatus::Offline(offline) => StreamerView::Offline(OfflineStreamerView {
            id: streamer.id.clone(),
            display_name: streamer.display_name.clone(),
            bindings,
            tier: streamer.tier,
            dgg: streamer.dgg,
            live: Default::default(),
            last_started_at: offline.last_started_at,
            last_ended_at: offline.last_ended_at,
            last_max_viewer_count: offline.last_max_viewer_count,
        }),
    }
}

/// Statuses (and live intelligence documents) for `streamers`, in one read.
pub async fn load_statuses(
    store: &Store,
    streamers: &[Streamer],
    with_intelligence: bool,
) -> Result<Vec<(StreamerStatus, Option<Value>)>, LiveError> {
    let ids: Vec<String> = streamers.iter().map(|s| s.id.clone()).collect();
    store
        .read(move |docs| {
            ids.iter()
                .map(|id| {
                    let status = docs.get::<StreamerStatus>(id)?.unwrap_or_else(|| {
                        StreamerStatus::Offline(crate::status::OfflineStatus::never_live(
                            id.clone(),
                        ))
                    });
                    let intelligence = if with_intelligence && status.is_live() {
                        docs.get_doc(&pk::<IntelligenceDocKey>(id)?)?
                            .map(|doc| js_to_json(&doc))
                    } else {
                        None
                    };
                    Ok((status, intelligence))
                })
                .collect()
        })
        .await
        .map_err(LiveError::persistence("read streamer status"))
}

/// Live first (primary tier, then hottest), then offline by most recent end.
pub async fn display_views(
    store: &Store,
    streamers: &[Streamer],
) -> Result<Vec<StreamerView>, LiveError> {
    let statuses = load_statuses(store, streamers, true).await?;
    let mut live = Vec::new();
    let mut offline = Vec::new();
    for (streamer, (status, intelligence)) in streamers.iter().zip(statuses) {
        let view = streamer_view(streamer, &status, intelligence);
        match &view {
            StreamerView::Live(entry) => {
                let rank = LiveRank {
                    tier: entry.tier,
                    viewer_count: entry.viewer_count,
                    max_viewer_count: entry.max_viewer_count,
                    ordering_viewer_count: streamer.ordering_viewer_count(),
                };
                live.push((rank, view));
            }
            StreamerView::Offline(_) => offline.push(view),
        }
    }
    sort_live_display(&mut live, |(rank, _)| *rank);
    sort_offline_display(&mut offline, |view| match view {
        StreamerView::Offline(entry) => entry.last_ended_at,
        StreamerView::Live(_) => None,
    });
    Ok(live
        .into_iter()
        .map(|(_, view)| view)
        .chain(offline)
        .collect())
}

/// The MCP-facing summary of one streamer.
pub fn livestream_summary(streamer: &Streamer, status: &StreamerStatus) -> LivestreamSummary {
    let bindings = streamer.bindings.iter().map(binding_view).collect();
    let discovery_source = streamer.discovery_source.map(|source| match source {
        DiscoverySource::Dgg => "dgg".to_owned(),
    });
    match status {
        StreamerStatus::Offline(offline) => LivestreamSummary {
            id: streamer.id.clone(),
            display_name: streamer.display_name.clone(),
            tier: streamer.tier,
            bindings,
            dgg: streamer.dgg,
            live: false,
            title: None,
            category: None,
            viewer_count: None,
            max_viewer_count: offline.last_max_viewer_count,
            started_at: None,
            last_started_at: offline.last_started_at,
            last_ended_at: offline.last_ended_at,
            primary: None,
            sources: Vec::new(),
            discovery_source,
        },
        StreamerStatus::Live(live) => LivestreamSummary {
            id: streamer.id.clone(),
            display_name: streamer.display_name.clone(),
            tier: streamer.tier,
            bindings,
            dgg: streamer.dgg,
            live: true,
            title: Some(live.primary_title.clone()),
            category: live.category.clone(),
            viewer_count: live.viewer_count,
            max_viewer_count: Some(live.max_viewer_count),
            started_at: Some(live.started_at),
            last_started_at: None,
            last_ended_at: None,
            primary: Some(binding_view(&live.primary)),
            sources: live
                .sources
                .iter()
                .flatten()
                .map(|source| LivestreamSourceSummary {
                    platform: source.platform.as_str().to_owned(),
                    username: source.username.clone(),
                    title: source.title.clone(),
                    viewer_count: source.viewer_count,
                    category: source.category.clone(),
                })
                .collect(),
            discovery_source,
        },
    }
}

/// The persisted status as a wire view.
pub fn status_view(status: &StreamerStatus) -> StreamerStatusView {
    match status {
        StreamerStatus::Live(live) => StreamerStatusView {
            streamer_id: live.streamer_id.clone(),
            is_live: true,
            primary: Some(binding_view(&live.primary)),
            primary_title: Some(live.primary_title.clone()),
            started_at: Some(live.started_at),
            max_viewer_count: Some(live.max_viewer_count),
            viewer_count: live.viewer_count,
            sources: live
                .sources
                .as_ref()
                .map(|s| s.iter().map(source_view).collect()),
            category: live.category.clone(),
            last_ended_at: None,
            last_started_at: None,
            last_max_viewer_count: None,
        },
        StreamerStatus::Offline(offline) => StreamerStatusView {
            streamer_id: offline.streamer_id.clone(),
            is_live: false,
            primary: None,
            primary_title: None,
            started_at: None,
            max_viewer_count: None,
            viewer_count: None,
            sources: None,
            category: None,
            last_ended_at: offline.last_ended_at,
            last_started_at: offline.last_started_at,
            last_max_viewer_count: offline.last_max_viewer_count,
        },
    }
}

fn bucket_view(bucket: &DailyBucket) -> DailyViewerBucket {
    DailyViewerBucket {
        date: bucket.date.clone(),
        max_viewers: bucket.max_viewers,
        timestamp: bucket.timestamp,
    }
}

/// `GET /api/streamers/:id/metrics` body.
pub fn metrics_view(
    metrics: &ViewerMetrics,
    platforms: &[PlatformViewerMetrics],
) -> StreamerMetricsResponse {
    StreamerMetricsResponse {
        daily_buckets: metrics.daily_buckets.iter().map(bucket_view).collect(),
        all_time_max: metrics.all_time_max,
        all_time_max_timestamp: metrics.all_time_max_timestamp,
        platforms: platforms
            .iter()
            .map(|platform| PlatformMetricsView {
                platform: platform.platform.clone(),
                username: platform.username.clone(),
                daily_buckets: platform.daily_buckets.iter().map(bucket_view).collect(),
                all_time_max: platform.all_time_max,
                all_time_max_timestamp: platform.all_time_max_timestamp,
            })
            .collect(),
    }
}

pub fn session_view(session: &StreamSession) -> StreamSessionView {
    StreamSessionView {
        started_at: session.started_at,
        ended_at: session.ended_at,
        duration_ms: session.duration_ms,
        peak_viewers: session.peak_viewers,
        title: session.title.clone(),
        platform: session.platform.as_str().to_owned(),
        username: session.username.clone(),
    }
}
