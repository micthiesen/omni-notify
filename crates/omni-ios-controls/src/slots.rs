//! The four live control slots: primary first, then
//! hottest, ties in channels.json order.

use omni_api::ios::{IOS_CONTROL_SLOT_COUNT, LiveSlotState};
use omni_live::Streamer;
use omni_live::display::load_statuses;
use omni_live::display_order::{LiveRank, sort_live_display};
use omni_live::error::LiveError;
use omni_live::status::StreamerStatus;
use omni_store::Store;
use serde_json::json;

/// Slot states for `streamers` at `now`.
pub async fn build_live_control_slots(
    store: &Store,
    streamers: &[Streamer],
    home_url: &str,
    now: i64,
) -> Result<Vec<LiveSlotState>, LiveError> {
    let statuses = load_statuses(store, streamers, false).await?;
    let mut ranked: Vec<(LiveRank, LiveSlotState)> = streamers
        .iter()
        .zip(statuses)
        .filter_map(|(streamer, (status, _))| {
            let StreamerStatus::Live(live) = status else {
                return None;
            };
            let rank = LiveRank {
                tier: streamer.tier,
                viewer_count: live.viewer_count,
                max_viewer_count: live.max_viewer_count,
                ordering_viewer_count: streamer.ordering_viewer_count(),
            };
            let slot = LiveSlotState {
                slot: 0,
                is_live: true,
                streamer_id: Some(streamer.id.clone()),
                display_name: streamer.display_name.clone(),
                title: Some(live.primary_title.clone()),
                platform: Some(live.primary.platform.as_str().to_owned()),
                url: live.primary.watch_url(),
                viewer_count: live.viewer_count,
                started_at: Some(live.started_at),
                updated_at: now,
            };
            Some((rank, slot))
        })
        .collect();
    sort_live_display(&mut ranked, |(rank, _)| *rank);
    let mut live = ranked.into_iter().map(|(_, slot)| slot);
    Ok((1..=IOS_CONTROL_SLOT_COUNT)
        .map(|index| match live.next() {
            Some(slot) => LiveSlotState {
                slot: index,
                ..slot
            },
            None => LiveSlotState {
                slot: index,
                is_live: false,
                streamer_id: None,
                display_name: "Nobody Live".to_owned(),
                title: None,
                platform: None,
                url: home_url.to_owned(),
                viewer_count: None,
                started_at: None,
                updated_at: now,
            },
        })
        .collect())
}

/// sha256 of `JSON.stringify` over the user-visible/control-action state only
/// (never the timestamp, viewers or uptime). Persisted as `lastDeliveredHash`.
pub fn live_control_slot_hash(slot: &LiveSlotState) -> String {
    let state = json!({
        "slot": slot.slot,
        "isLive": slot.is_live,
        "streamerId": slot.streamer_id,
        "displayName": slot.display_name,
        "title": slot.title,
        "url": slot.url,
    });
    omni_core::digest::sha256_hex(omni_core::js::json_stringify(&state))
}
