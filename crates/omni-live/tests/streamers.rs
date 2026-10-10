//! Aggregate streamers over platform bindings.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_api::streamers::StreamerTier;
use omni_live::streamers::{
    BACKGROUND_POLL_FACTOR, Streamer, drop_platform_bindings, is_streamer_due, normalize_id,
};
use omni_live::{Platform, PlatformBinding};

fn b(platform: Platform, username: &str) -> PlatformBinding {
    PlatformBinding::new(platform, username)
}

#[test]
fn lowercases_and_trims_display_names() {
    assert_eq!(normalize_id("  Destiny  "), "destiny");
    assert_eq!(normalize_id("DESTINY"), "destiny");
}

fn streamer(id: &str, bindings: Vec<PlatformBinding>) -> Streamer {
    Streamer::new(id, "S", bindings, StreamerTier::Primary)
}

#[test]
fn removes_only_the_target_platforms_bindings_keeping_others() {
    let (result, dropped) = drop_platform_bindings(
        vec![streamer(
            "s",
            vec![b(Platform::YouTube, "@a"), b(Platform::Kick, "a")],
        )],
        Platform::Kick,
    );
    assert!(dropped);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].bindings, vec![b(Platform::YouTube, "@a")]);
}

#[test]
fn drops_a_streamer_entirely_when_its_only_binding_is_the_target_platform() {
    let (result, dropped) = drop_platform_bindings(
        vec![
            streamer("kickonly", vec![b(Platform::Kick, "a")]),
            streamer(
                "mixed",
                vec![b(Platform::YouTube, "@b"), b(Platform::Kick, "b")],
            ),
        ],
        Platform::Kick,
    );
    assert!(dropped);
    let ids: Vec<&str> = result.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["mixed"]);
}

#[test]
fn reports_dropped_any_false_and_returns_streamers_unchanged_when_the_platform_isnt_present() {
    let streamers = vec![streamer("s", vec![b(Platform::YouTube, "@a")])];
    let (result, dropped) = drop_platform_bindings(streamers.clone(), Platform::Kick);
    assert!(!dropped);
    assert_eq!(result, streamers);
}

#[test]
fn polls_primary_streamers_on_every_tick() {
    for tick in 0..6 {
        assert!(is_streamer_due(StreamerTier::Primary, tick));
    }
}

#[test]
fn polls_background_streamers_on_the_startup_tick() {
    assert!(is_streamer_due(StreamerTier::Background, 0));
}

#[test]
fn skips_background_streamers_between_poll_factor_ticks() {
    assert!(!is_streamer_due(StreamerTier::Background, 1));
    assert!(!is_streamer_due(StreamerTier::Background, 2));
    assert!(!is_streamer_due(
        StreamerTier::Background,
        BACKGROUND_POLL_FACTOR + 1
    ));
}

#[test]
fn polls_background_streamers_on_every_poll_factor_th_tick() {
    assert!(is_streamer_due(
        StreamerTier::Background,
        BACKGROUND_POLL_FACTOR
    ));
    assert!(is_streamer_due(
        StreamerTier::Background,
        BACKGROUND_POLL_FACTOR * 2
    ));
}

#[test]
fn trims_display_names_with_javascript_whitespace_rules() {
    use omni_live::streamers::{js_trim, normalize_id};
    // JS trims U+FEFF but keeps U+0085, the reverse of `str::trim`.
    assert_eq!(normalize_id("\u{FEFF} Destiny \u{3000}"), "destiny");
    assert_eq!(js_trim("\u{0085}x\u{0085}"), "\u{0085}x\u{0085}");
}
