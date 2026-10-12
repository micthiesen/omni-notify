//! The On Air and Deliveries widget timelines and the On Air content hash
//! that decides widget pushes.

use std::collections::HashMap;

use omni_api::ios::{
    DeliveriesWidgetResponse, OnAirLive, OnAirRecent, OnAirResponse, WidgetDelivery,
};
use omni_api::parcels::ParcelsResponse;
use omni_api::streamers::StreamerTier;
use omni_live::Streamer;
use omni_live::deep_link::app_url;
use omni_live::display::load_statuses;
use omni_live::display_order::{LiveRank, sort_live_display, sort_offline_display};
use omni_live::error::LiveError;
use omni_live::status::{LiveStatus, StreamerStatus};
use omni_store::Store;
use serde_json::{Value, json};

/// Live entries in the On Air widget.
pub const ON_AIR_LIVE_LIMIT: usize = 8;
/// Recently ended primary streamers in the On Air empty state.
pub const ON_AIR_RECENT_LIMIT: usize = 4;
/// Entries in the Deliveries widget.
pub const DELIVERIES_LIMIT: usize = 8;
/// Completed deliveries shown after the active ones.
pub const COMPLETED_LIMIT: usize = 3;

/// The current session's intelligence headline from the raw
/// `livestream-intelligence` document, if it was produced during this session.
fn headline(intelligence: Option<&Value>, live: &LiveStatus) -> Option<String> {
    let semantic = intelligence?.get("semantic")?;
    let updated_at = semantic.get("updatedAt")?.as_f64()?;
    #[allow(clippy::cast_precision_loss)]
    let current = updated_at >= live.started_at as f64;
    let text = semantic.get("headline")?.as_str()?.trim();
    (current && !text.is_empty()).then(|| text.to_owned())
}

/// The On Air timeline for `streamers` (the roster, including discoveries)
/// at `now`. `avatars` maps streamer id to avatar URL.
pub async fn build_on_air(
    store: &Store,
    streamers: &[Streamer],
    avatars: &HashMap<String, String>,
    now: i64,
) -> Result<OnAirResponse, LiveError> {
    let statuses = load_statuses(store, streamers, true).await?;
    let mut live = Vec::new();
    let mut recent = Vec::new();
    for (streamer, (status, intelligence)) in streamers.iter().zip(statuses) {
        match status {
            StreamerStatus::Live(status) => {
                let rank = LiveRank {
                    tier: streamer.tier,
                    viewer_count: status.viewer_count,
                    max_viewer_count: status.max_viewer_count,
                    ordering_viewer_count: streamer.ordering_viewer_count(),
                };
                let entry = OnAirLive {
                    id: streamer.id.clone(),
                    name: streamer.display_name.clone(),
                    tier: streamer.tier,
                    platform: status.primary.platform.as_str().to_owned(),
                    title: Some(status.primary_title.clone()).filter(|t| !t.is_empty()),
                    category: status.category.clone(),
                    viewers: status.viewer_count,
                    peak_viewers: Some(status.max_viewer_count).filter(|peak| *peak > 0),
                    started_at: Some(status.started_at),
                    avatar_url: avatars.get(&streamer.id).cloned(),
                    watch_url: status.primary.watch_url(),
                    app_url: app_url(status.primary.platform, &status.primary.username),
                    headline: headline(intelligence.as_ref(), &status),
                };
                live.push((rank, entry));
            }
            StreamerStatus::Offline(offline) => {
                let (Some(ended_at), Some(binding)) =
                    (offline.last_ended_at, streamer.bindings.first())
                else {
                    continue;
                };
                if streamer.tier != StreamerTier::Primary || streamer.discovery_source.is_some() {
                    continue;
                }
                recent.push(OnAirRecent {
                    id: streamer.id.clone(),
                    name: streamer.display_name.clone(),
                    platform: binding.platform.as_str().to_owned(),
                    ended_at,
                    avatar_url: avatars.get(&streamer.id).cloned(),
                    watch_url: binding.watch_url(),
                });
            }
        }
    }
    sort_live_display(&mut live, |(rank, _)| *rank);
    sort_offline_display(&mut recent, |r| Some(r.ended_at));
    let total_viewers = live.iter().filter_map(|(_, e)| e.viewers).sum();
    Ok(OnAirResponse {
        generated_at: now,
        live_count: live.len() as u64,
        total_viewers,
        live: live
            .into_iter()
            .take(ON_AIR_LIVE_LIMIT)
            .map(|(_, entry)| entry)
            .collect(),
        recent: recent.into_iter().take(ON_AIR_RECENT_LIMIT).collect(),
    })
}

/// sha256 of `JSON.stringify` over what the On Air widget shows that should
/// trigger a reload: the set of listed live streamers (sorted by id) with
/// title and platform. Never the viewer-ranked order, viewer counts, avatars
/// or timestamps, so two close streamers trading places do not push every
/// tick; the widget picks up the new order on its next timeline reload.
pub fn on_air_hash(on_air: &OnAirResponse) -> String {
    let mut entries: Vec<&omni_api::ios::OnAirLive> = on_air.live.iter().collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    let state: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "title": entry.title,
                "platform": entry.platform,
            })
        })
        .collect();
    omni_core::digest::sha256_hex(omni_core::js::json_stringify(&Value::Array(state)))
}

/// The Deliveries timeline from the cached Parcel view (`None` when the
/// parcel subsystem provides none).
pub fn deliveries_widget(cached: Option<ParcelsResponse>, now: i64) -> DeliveriesWidgetResponse {
    let Some(cached) = cached else {
        return DeliveriesWidgetResponse {
            generated_at: now,
            configured: false,
            fetched_at: None,
            active_count: 0,
            deliveries: Vec::new(),
        };
    };
    let (active, completed): (Vec<_>, Vec<_>) =
        cached.deliveries.into_iter().partition(|d| d.active);
    let deliveries = active
        .into_iter()
        .chain(completed.into_iter().take(COMPLETED_LIMIT))
        .take(DELIVERIES_LIMIT)
        .map(|delivery| {
            let last = delivery.events.first();
            WidgetDelivery {
                tracking_number: delivery.tracking_number.clone(),
                carrier: delivery.carrier_name.clone(),
                description: delivery.description.clone(),
                status: delivery.status,
                status_label: delivery.status.label().to_owned(),
                active: delivery.active,
                expected: delivery.expected.clone(),
                expected_end: delivery.expected_end.clone(),
                last_event: last.map(|e| e.description.clone()),
                last_event_at: last.and_then(|e| e.date.clone()),
                last_location: last.and_then(|e| e.location.clone()),
            }
        })
        .collect();
    DeliveriesWidgetResponse {
        generated_at: now,
        configured: cached.configured,
        fetched_at: cached.fetched_at,
        active_count: cached.active_count,
        deliveries,
    }
}
