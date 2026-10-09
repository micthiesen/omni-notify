//! Streamers, viewer metrics, sessions and trigger channels (WP04).
//!
//! `GET /api/streamers`, the `streamers` array of the dashboard snapshot,
//! `GET /api/trigger-channels`, `GET /api/streamers/:id/metrics` and
//! `GET /api/streamers/:id/sessions`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::common::{Ms, encode_uri_component};

/// A JSON `true` literal (the `live` discriminant of a live streamer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveTrue;

/// A JSON `false` literal (the `live` discriminant of an offline streamer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveFalse;

macro_rules! bool_literal {
    ($ty:ident, $value:literal) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bool($value)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                if bool::deserialize(deserializer)? == $value {
                    Ok($ty)
                } else {
                    Err(serde::de::Error::custom(concat!("expected live: ", $value)))
                }
            }
        }
    };
}

bool_literal!(LiveTrue, true);
bool_literal!(LiveFalse, false);

/// `"primary" | "background"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamerTier {
    #[default]
    Primary,
    Background,
}

/// One platform account of a streamer, with its watch URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerBinding {
    pub platform: String,
    pub username: String,
    pub url: String,
}

/// Presence in Destiny.gg's current embeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DggPresence {
    pub hosted: bool,
    pub viewers: Option<i64>,
}

/// One live platform observation contributing to `viewerCount`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSourceView {
    pub platform: String,
    pub username: String,
    pub title: String,
    pub viewer_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// A live streamer (`live: true`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStreamerView {
    pub id: String,
    pub display_name: String,
    pub bindings: Vec<StreamerBinding>,
    pub tier: StreamerTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dgg: Option<DggPresence>,
    pub live: LiveTrue,
    pub title: String,
    pub started_at: Ms,
    pub max_viewer_count: i64,
    /// Current summed viewer count; `null` for rows written before it existed.
    pub viewer_count: Option<i64>,
    pub sources: Vec<LiveSourceView>,
    pub category: Option<String>,
    pub primary: StreamerBinding,
    /// The raw `livestream-intelligence` document (WP05 owns its shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intelligence: Option<serde_json::Value>,
}

/// An offline streamer (`live: false`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineStreamerView {
    pub id: String,
    pub display_name: String,
    pub bindings: Vec<StreamerBinding>,
    pub tier: StreamerTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dgg: Option<DggPresence>,
    pub live: LiveFalse,
    pub last_started_at: Option<Ms>,
    pub last_ended_at: Option<Ms>,
    pub last_max_viewer_count: Option<i64>,
}

/// `StreamerView = LiveStreamer | OfflineStreamer`, discriminated by `live`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamerView {
    Live(LiveStreamerView),
    Offline(OfflineStreamerView),
}

impl StreamerView {
    pub fn id(&self) -> &str {
        match self {
            StreamerView::Live(live) => &live.id,
            StreamerView::Offline(offline) => &offline.id,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, StreamerView::Live(_))
    }
}

/// `GET /api/streamers`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamersResponse {
    pub streamers: Vec<StreamerView>,
}

/// `"youtube" | "twitch" | "kick"` for the Homebridge trigger plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerChannelType {
    Youtube,
    Twitch,
    Kick,
}

/// One switch of the homebridge-stream-triggers plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerChannel {
    pub key: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub kind: TriggerChannelType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// `GET /api/trigger-channels`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerChannelsResponse {
    pub channels: Vec<TriggerChannel>,
}

/// One UTC day's peak viewer count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyViewerBucket {
    /// `YYYY-MM-DD` (UTC).
    pub date: String,
    pub max_viewers: i64,
    pub timestamp: Ms,
}

/// Per-platform-account viewer history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformViewerMetrics {
    pub platform: String,
    pub username: String,
    pub daily_buckets: Vec<DailyViewerBucket>,
    pub all_time_max: i64,
    pub all_time_max_timestamp: Ms,
}

/// `GET /api/streamers/:id/metrics`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerMetricsResponse {
    pub daily_buckets: Vec<DailyViewerBucket>,
    pub all_time_max: i64,
    pub all_time_max_timestamp: Ms,
    pub platforms: Vec<PlatformViewerMetrics>,
}

/// One completed live session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSessionView {
    pub started_at: Ms,
    pub ended_at: Ms,
    pub duration_ms: i64,
    pub peak_viewers: i64,
    pub title: String,
    pub platform: String,
    pub username: String,
}

/// `GET /api/streamers/:id/sessions` (newest first).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSessionsResponse {
    pub sessions: Vec<StreamSessionView>,
}

/// A live source in the MCP-facing summary (`category` always present).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamSourceSummary {
    pub platform: String,
    pub username: String,
    pub title: String,
    pub viewer_count: Option<i64>,
    pub category: Option<String>,
}

/// One streamer as the `livestreams_list` / `livestream_get` MCP tools show it
/// (`LiveDirectory::streamers`): every field present, `null` when unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamSummary {
    pub id: String,
    pub display_name: String,
    pub tier: StreamerTier,
    pub bindings: Vec<StreamerBinding>,
    pub dgg: Option<DggPresence>,
    pub live: bool,
    pub title: Option<String>,
    pub category: Option<String>,
    pub viewer_count: Option<i64>,
    pub max_viewer_count: Option<i64>,
    pub started_at: Option<Ms>,
    pub last_started_at: Option<Ms>,
    pub last_ended_at: Option<Ms>,
    pub primary: Option<StreamerBinding>,
    pub sources: Vec<LivestreamSourceSummary>,
    /// `"dgg"` for a transient Destiny.gg discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_source: Option<String>,
}

/// The persisted aggregate state of one streamer (`LiveDirectory::statuses`),
/// Dates as epoch ms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerStatusView {
    pub streamer_id: String,
    pub is_live: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<StreamerBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_viewer_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<LiveSourceView>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ended_at: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_started_at: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_max_viewer_count: Option<i64>,
}

/// `LiveDirectory::details`: everything `livestream_get` can include.
/// `sessions` are oldest to newest and unbounded; the caller filters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivestreamDetails {
    pub livestream: LivestreamSummary,
    pub metrics: StreamerMetricsResponse,
    pub sessions: Vec<StreamSessionView>,
}

/// Path builders for the WP04 routes.
pub mod paths {
    use super::encode_uri_component;

    pub const STREAMERS: &str = "/api/streamers";
    pub const TRIGGER_CHANNELS: &str = "/api/trigger-channels";

    /// `GET /api/streamers/:id/metrics`.
    pub fn streamer_metrics(id: &str) -> String {
        format!("/api/streamers/{}/metrics", encode_uri_component(id))
    }

    /// `GET /api/streamers/:id/sessions`.
    pub fn streamer_sessions(id: &str) -> String {
        format!("/api/streamers/{}/sessions", encode_uri_component(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_and_offline_views_round_trip_through_the_live_discriminant() {
        let live = json!({
            "id": "destiny",
            "displayName": "Destiny",
            "bindings": [{"platform": "kick", "username": "destiny", "url": "https://kick.com/destiny"}],
            "tier": "primary",
            "live": true,
            "title": "t",
            "startedAt": 1,
            "maxViewerCount": 10,
            "viewerCount": null,
            "sources": [{"platform": "kick", "username": "destiny", "title": "t", "viewerCount": null}],
            "category": null,
            "primary": {"platform": "kick", "username": "destiny", "url": "https://kick.com/destiny"}
        });
        let offline = json!({
            "id": "jerma",
            "displayName": "Jerma",
            "bindings": [],
            "tier": "background",
            "dgg": {"hosted": false, "viewers": null},
            "live": false,
            "lastStartedAt": null,
            "lastEndedAt": 5,
            "lastMaxViewerCount": null
        });
        let parsed: Vec<StreamerView> =
            serde_json::from_value(json!([live.clone(), offline.clone()])).unwrap();
        assert!(parsed[0].is_live());
        assert!(!parsed[1].is_live());
        assert_eq!(parsed[1].id(), "jerma");
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            json!([live, offline])
        );
    }

    #[test]
    fn trigger_channels_omit_absent_urls() {
        let channel = TriggerChannel {
            key: "jerma".into(),
            display_name: "Jerma".into(),
            kind: TriggerChannelType::Twitch,
            url: None,
        };
        assert_eq!(
            serde_json::to_value(channel).unwrap(),
            json!({"key": "jerma", "displayName": "Jerma", "type": "twitch"})
        );
    }

    #[test]
    fn path_builders_encode_ids() {
        assert_eq!(
            paths::streamer_metrics("dgg:kick:a b"),
            "/api/streamers/dgg%3Akick%3Aa%20b/metrics"
        );
    }
}
