//! Aggregate live transitions.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_live::platform::{FetchedLive, FetchedStatus};
use omni_live::status::{LiveSource, LiveStatus, OfflineStatus, StreamerStatus};
use omni_live::transitions::{BindingFetchResult, TickDecision, decide_transition};
use omni_live::{Platform, PlatformBinding};
use omni_store::cbor::Extra;

const NOW: i64 = 1_776_384_000_000; // 2026-04-17T00:00:00Z
const EARLIER: i64 = 1_776_297_600_000; // 2026-04-16T00:00:00Z

fn yt() -> PlatformBinding {
    PlatformBinding::new(Platform::YouTube, "@yt")
}
fn tw() -> PlatformBinding {
    PlatformBinding::new(Platform::Twitch, "tw")
}
fn ki() -> PlatformBinding {
    PlatformBinding::new(Platform::Kick, "ki")
}

fn live(title: &str, viewers: i64, category: Option<&str>) -> FetchedStatus {
    FetchedStatus::Live(FetchedLive {
        title: title.into(),
        viewer_count: Some(viewers),
        category: category.map(str::to_owned),
        started_at: None,
    })
}

fn live_started(started_at: &str) -> FetchedStatus {
    FetchedStatus::Live(FetchedLive {
        title: "t".into(),
        viewer_count: Some(100),
        category: None,
        started_at: Some(started_at.into()),
    })
}

fn offline() -> FetchedStatus {
    FetchedStatus::Offline
}

fn unknown(error: &str) -> FetchedStatus {
    FetchedStatus::unknown(error)
}

fn r(binding: PlatformBinding, status: FetchedStatus) -> BindingFetchResult {
    BindingFetchResult { binding, status }
}

fn offline_status() -> StreamerStatus {
    StreamerStatus::Offline(OfflineStatus::never_live("s"))
}

fn live_status(
    primary: PlatformBinding,
    title: &str,
    max: i64,
    category: Option<&str>,
) -> StreamerStatus {
    StreamerStatus::Live(LiveStatus {
        streamer_id: "s".into(),
        primary,
        primary_title: title.into(),
        started_at: EARLIER,
        max_viewer_count: max,
        viewer_count: None,
        sources: None,
        category: category.map(str::to_owned),
        extra: Extra::new(),
    })
}

#[test]
fn returns_all_unknown_when_every_binding_errors() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[r(yt(), unknown("e1")), r(ki(), unknown("e2"))],
        NOW,
    );
    assert_eq!(
        d,
        TickDecision::AllUnknown {
            errors: vec!["e1".into(), "e2".into()]
        }
    );
}

#[test]
fn keeps_previous_state_when_partial_unknown_and_no_lives() {
    let d = decide_transition(
        "s",
        &live_status(ki(), "prev", 50, None),
        &[r(yt(), unknown("boom")), r(ki(), offline())],
        NOW,
    );
    assert_eq!(d, TickDecision::NoChange);
}

#[test]
fn fires_went_live_when_offline_to_any_live_priority_tiebreak() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[
            r(yt(), live("yt-title", 10, None)),
            r(ki(), live("kick-title", 50, None)),
        ],
        NOW,
    );
    let TickDecision::WentLive {
        next,
        summed_viewer_count,
    } = d
    else {
        panic!("went-live")
    };
    assert_eq!(next.primary, yt());
    assert_eq!(next.primary_title, "yt-title");
    assert_eq!(next.max_viewer_count, 60);
    assert_eq!(summed_viewer_count, 60);
    assert_eq!(next.started_at, NOW);
    assert_eq!(next.viewer_count, Some(60));
    assert_eq!(
        next.sources,
        Some(vec![
            LiveSource {
                platform: Platform::YouTube,
                username: "@yt".into(),
                title: "yt-title".into(),
                viewer_count: Some(10),
                category: None
            },
            LiveSource {
                platform: Platform::Kick,
                username: "ki".into(),
                title: "kick-title".into(),
                viewer_count: Some(50),
                category: None
            },
        ])
    );
}

#[test]
fn sets_category_from_the_primary_bindings_live_status_on_went_live() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[r(ki(), live("kick-title", 50, Some("Just Chatting")))],
        NOW,
    );
    let TickDecision::WentLive { next, .. } = d else {
        panic!("went-live")
    };
    assert_eq!(next.category.as_deref(), Some("Just Chatting"));
}

#[test]
fn uses_a_valid_source_reported_start_time_on_went_live() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[r(ki(), live_started("2026-04-16T23:30:00Z"))],
        NOW,
    );
    let TickDecision::WentLive { next, .. } = d else {
        panic!("went-live")
    };
    assert_eq!(next.started_at, NOW - 30 * 60_000);
}

#[test]
fn ignores_an_invalid_or_future_source_reported_start_time() {
    for started_at in ["not-a-date", "2026-04-18T00:00:00Z"] {
        let d = decide_transition(
            "s",
            &offline_status(),
            &[r(ki(), live_started(started_at))],
            NOW,
        );
        let TickDecision::WentLive { next, .. } = d else {
            panic!("went-live")
        };
        assert_eq!(next.started_at, NOW, "{started_at}");
    }
}

#[test]
fn leaves_category_undefined_when_the_primary_binding_reports_none_e_g_youtube() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[r(yt(), live("yt-title", 10, None))],
        NOW,
    );
    let TickDecision::WentLive { next, .. } = d else {
        panic!("went-live")
    };
    assert_eq!(next.category, None);
}

#[test]
fn fires_went_offline_when_live_to_all_confirmed_offline() {
    let d = decide_transition(
        "s",
        &live_status(ki(), "t", 200, None),
        &[r(yt(), offline()), r(ki(), offline())],
        NOW,
    );
    let TickDecision::WentOffline {
        previous_live,
        next,
    } = d
    else {
        panic!("went-offline")
    };
    assert_eq!(previous_live.primary, ki());
    assert_eq!(next.last_ended_at, Some(NOW));
    assert_eq!(next.last_started_at, Some(EARLIER));
    assert_eq!(next.last_max_viewer_count, Some(200));
}

#[test]
fn promotes_youtube_over_a_live_kick_primary() {
    let d = decide_transition(
        "s",
        &live_status(ki(), "kick-old", 50, None),
        &[
            r(yt(), live("yt", 10, None)),
            r(ki(), live("kick-new-title", 100, Some("Elden Ring"))),
        ],
        NOW,
    );
    let TickDecision::StillLive {
        next,
        primary_switched,
        title_changed,
        ..
    } = d
    else {
        panic!("still-live")
    };
    assert_eq!(next.primary, yt());
    assert!(primary_switched);
    assert!(!title_changed);
    assert_eq!(next.primary_title, "yt");
    assert_eq!(next.viewer_count, Some(110));
    assert_eq!(next.category, None);
}

#[test]
fn keeps_a_non_kick_primary_sticky_while_it_remains_live() {
    let d = decide_transition(
        "s",
        &live_status(tw(), "twitch-old", 50, None),
        &[
            r(yt(), live("yt", 10, None)),
            r(tw(), live("twitch-new-title", 100, Some("Elden Ring"))),
        ],
        NOW,
    );
    let TickDecision::StillLive {
        next,
        primary_switched,
        title_changed,
        ..
    } = d
    else {
        panic!("still-live")
    };
    assert_eq!(next.primary, tw());
    assert!(!primary_switched);
    assert!(title_changed);
    assert_eq!(next.primary_title, "twitch-new-title");
    assert_eq!(next.category.as_deref(), Some("Elden Ring"));
}

#[test]
fn re_elects_primary_when_the_previous_primary_drops_and_other_bindings_remain_live() {
    let d = decide_transition(
        "s",
        &live_status(ki(), "kick-title", 50, Some("Slots")),
        &[r(yt(), live("yt-title", 10, None)), r(ki(), offline())],
        NOW,
    );
    let TickDecision::StillLive {
        next,
        primary_switched,
        title_changed,
        ..
    } = d
    else {
        panic!("still-live")
    };
    assert_eq!(next.primary, yt());
    assert!(primary_switched);
    assert!(!title_changed);
    assert_eq!(next.category, None);
}

#[test]
fn retains_the_last_known_category_when_a_still_live_tick_reports_none() {
    let d = decide_transition(
        "s",
        &live_status(ki(), "t", 50, Some("Elden Ring")),
        &[r(ki(), live("t", 60, None))],
        NOW,
    );
    let TickDecision::StillLive { next, .. } = d else {
        panic!("still-live")
    };
    assert_eq!(next.category.as_deref(), Some("Elden Ring"));
}

#[test]
fn does_not_fire_title_change_when_primary_unchanged_and_title_unchanged() {
    let d = decide_transition(
        "s",
        &live_status(ki(), "same", 50, None),
        &[r(ki(), live("same", 60, None))],
        NOW,
    );
    let TickDecision::StillLive {
        next,
        primary_switched,
        title_changed,
        ..
    } = d
    else {
        panic!("still-live")
    };
    assert!(!title_changed);
    assert!(!primary_switched);
    assert_eq!(next.max_viewer_count, 60);
}

#[test]
fn sums_viewer_counts_across_live_bindings() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[
            r(yt(), live("t", 1200, None)),
            r(tw(), live("t", 800, None)),
            r(ki(), live("t", 500, None)),
        ],
        NOW,
    );
    let TickDecision::WentLive {
        next,
        summed_viewer_count,
    } = d
    else {
        panic!("went-live")
    };
    assert_eq!(summed_viewer_count, 2500);
    assert_eq!(next.primary, yt());
}

#[test]
fn returns_no_change_when_already_offline_and_all_bindings_still_offline() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[r(yt(), offline()), r(ki(), offline())],
        NOW,
    );
    assert_eq!(d, TickDecision::NoChange);
}

#[test]
fn treats_unknown_and_live_as_live_keeps_streamer_live() {
    let d = decide_transition(
        "s",
        &offline_status(),
        &[r(yt(), unknown("boom")), r(ki(), live("kick", 100, None))],
        NOW,
    );
    let TickDecision::WentLive {
        next,
        summed_viewer_count,
    } = d
    else {
        panic!("went-live")
    };
    assert_eq!(next.primary, ki());
    assert_eq!(summed_viewer_count, 100);
}
