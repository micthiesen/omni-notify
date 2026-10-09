//! `livestream.status_changed` publications for the aggregate live edges.
//!
//! The dedup key is the streamer, session start and edge, so a went-offline
//! edge that is re-detected after a crash (the live row stays until the
//! offline write) keeps its event ID and is not delivered twice.

use omni_api::events::{
    LIVESTREAM_STATUS_CHANGED, LivestreamStatusChanged, LivestreamTransition, bounded_title,
};
use omni_runtime::ports::{EventPublication, Ports};
use serde_json::Value;

use crate::status::LiveStatus;
use crate::streamers::Streamer;

const LOG: &str = "LiveCheckTask";

/// The publication for one aggregate edge of `live`'s session.
pub fn status_changed(
    streamer: &Streamer,
    live: &LiveStatus,
    transition: LivestreamTransition,
    ended_at: Option<i64>,
    now: i64,
) -> Option<EventPublication> {
    let payload = LivestreamStatusChanged {
        streamer_id: streamer.id.clone(),
        display_name: bounded_title(&streamer.display_name).unwrap_or_else(|| streamer.id.clone()),
        transition,
        tier: streamer.tier,
        platform: live.primary.platform.as_str().to_owned(),
        title: bounded_title(&live.primary_title),
        started_at: omni_core::js::to_iso_string(live.started_at),
        ended_at: ended_at.map(omni_core::js::to_iso_string),
        viewer_count: match transition {
            LivestreamTransition::WentLive => live.viewer_count,
            LivestreamTransition::WentOffline => None,
        },
        max_viewer_count: Some(live.max_viewer_count),
    };
    let Ok(Value::Object(data)) = serde_json::to_value(payload) else {
        return None;
    };
    Some(EventPublication {
        name: LIVESTREAM_STATUS_CHANGED,
        dedup_key: format!(
            "{}:{}:{}",
            streamer.id,
            live.started_at,
            transition.as_str()
        ),
        occurred_at_ms: ended_at.unwrap_or(now),
        data,
    })
}

/// Publishes when MCP Events are enabled. A failure is logged and never
/// fails the tick: the edge's notification and state write stand on their own.
pub async fn publish(ports: Option<&Ports>, event: Option<EventPublication>) {
    let (Some(ports), Some(event)) = (ports, event) else {
        return;
    };
    if let Err(error) = ports.publish_event(&event).await {
        tracing::warn!(target: LOG, error = %error, "Livestream event not published");
    }
}
