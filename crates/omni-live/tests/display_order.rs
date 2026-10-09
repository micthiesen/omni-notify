//! Port of `src/live-check/displayOrder.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_api::streamers::StreamerTier::{Background, Primary};
use omni_live::display_order::{LiveRank, sort_live_display, sort_offline_display};

fn rank(
    id: &'static str,
    tier: omni_api::streamers::StreamerTier,
    viewers: Option<i64>,
    max: i64,
    ordering: Option<i64>,
) -> (&'static str, LiveRank) {
    (
        id,
        LiveRank {
            tier,
            viewer_count: viewers,
            max_viewer_count: max,
            ordering_viewer_count: ordering,
        },
    )
}

fn ids(items: &[(&'static str, LiveRank)]) -> Vec<&'static str> {
    items.iter().map(|(id, _)| *id).collect()
}

#[test]
fn keeps_primary_channels_ahead_of_hotter_background_channels() {
    let mut items = vec![
        rank("background", Background, Some(50_000), 50_000, None),
        rank("primary-cool", Primary, Some(10), 20, None),
        rank("primary-hot", Primary, Some(20), 30, None),
    ];
    sort_live_display(&mut items, |(_, r)| *r);
    assert_eq!(ids(&items), ["primary-hot", "primary-cool", "background"]);
}

#[test]
fn uses_session_peak_when_current_viewers_are_unavailable_and_preserves_ties() {
    let mut items = vec![
        rank("first-tie", Primary, None, 100, None),
        rank("second-tie", Primary, Some(100), 120, None),
        rank("cooler", Primary, None, 20, None),
    ];
    sort_live_display(&mut items, |(_, r)| *r);
    assert_eq!(ids(&items), ["first-tie", "second-tie", "cooler"]);
}

#[test]
fn uses_a_rank_only_viewer_override_without_replacing_actual_viewers() {
    let mut items = vec![
        rank(
            "dgg-large-platform",
            Background,
            Some(50_000),
            60_000,
            Some(12),
        ),
        rank("configured", Background, Some(80), 100, None),
        rank("dgg-popular", Background, Some(200), 300, Some(90)),
    ];
    sort_live_display(&mut items, |(_, r)| *r);
    assert_eq!(
        ids(&items),
        ["dgg-popular", "configured", "dgg-large-platform"]
    );
    assert_eq!(items[2].1.viewer_count, Some(50_000));
}

#[test]
fn orders_offline_channels_by_their_most_recent_end() {
    let mut items = vec![("never", None), ("older", Some(10)), ("newer", Some(20))];
    sort_offline_display(&mut items, |(_, ended)| *ended);
    let ids: Vec<&str> = items.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, ["newer", "older", "never"]);
}
