//! Port of `src/live-check/streamers.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_api::streamers::StreamerTier;
use omni_live::channels::{ChannelEntry, ChannelsConfig, Usernames};
use omni_live::streamers::{
    BACKGROUND_POLL_FACTOR, BuildStreamersError, Streamer, build_streamers, drop_platform_bindings,
    is_streamer_due, normalize_id,
};
use omni_live::{Platform, PlatformBinding};

fn one(name: &str) -> Option<Usernames> {
    Some(Usernames::One(name.to_owned()))
}

fn many(names: &[&str]) -> Option<Usernames> {
    Some(Usernames::Many(
        names.iter().map(|n| (*n).to_owned()).collect(),
    ))
}

fn config(entries: Vec<(&str, ChannelEntry)>) -> ChannelsConfig {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

fn b(platform: Platform, username: &str) -> PlatformBinding {
    PlatformBinding::new(platform, username)
}

#[test]
fn lowercases_and_trims_display_names() {
    assert_eq!(normalize_id("  Destiny  "), "destiny");
    assert_eq!(normalize_id("DESTINY"), "destiny");
}

#[test]
fn builds_bindings_from_an_entrys_platform_fields() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            youtube: one("@destiny2"),
            kick: one("destiny"),
            ..Default::default()
        },
    )]))
    .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "destiny");
    assert_eq!(result[0].display_name, "Destiny");
    assert_eq!(
        result[0].bindings,
        vec![
            b(Platform::YouTube, "@destiny2"),
            b(Platform::Kick, "destiny")
        ]
    );
}

#[test]
fn always_orders_bindings_youtube_twitch_kick_regardless_of_the_entrys_field_order() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            kick: one("destiny"),
            youtube: one("@destiny2"),
            twitch: one("destinytv"),
            ..Default::default()
        },
    )]))
    .unwrap();
    let platforms: Vec<Platform> = result[0].bindings.iter().map(|b| b.platform).collect();
    assert_eq!(
        platforms,
        [Platform::YouTube, Platform::Twitch, Platform::Kick]
    );
}

#[test]
fn accepts_a_string_array_for_multiple_usernames_on_one_platform() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            twitch: many(&["destiny", "destinyalt"]),
            ..Default::default()
        },
    )]))
    .unwrap();
    assert_eq!(
        result[0].bindings,
        vec![
            b(Platform::Twitch, "destiny"),
            b(Platform::Twitch, "destinyalt")
        ]
    );
}

#[test]
fn keeps_distinct_streamers_for_distinct_entries() {
    let result = build_streamers(&config(vec![
        (
            "Shroud",
            ChannelEntry {
                twitch: one("shroud"),
                ..Default::default()
            },
        ),
        (
            "Destiny",
            ChannelEntry {
                kick: one("destiny"),
                ..Default::default()
            },
        ),
    ]))
    .unwrap();
    let mut names: Vec<&str> = result.iter().map(|s| s.display_name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Destiny", "Shroud"]);
}

fn duplicate(entries: Vec<(&str, ChannelEntry)>) -> BuildStreamersError {
    build_streamers(&config(entries)).unwrap_err()
}

#[test]
fn throws_on_a_platform_binding_duplicated_across_entries() {
    let error = duplicate(vec![
        (
            "Shroud",
            ChannelEntry {
                twitch: one("shroud"),
                ..Default::default()
            },
        ),
        (
            "OtherName",
            ChannelEntry {
                twitch: one("shroud"),
                ..Default::default()
            },
        ),
    ]);
    assert!(error.to_string().contains("Duplicate platform binding"));
}

#[test]
fn throws_on_a_platform_binding_duplicated_within_one_entrys_array() {
    let error = duplicate(vec![(
        "Shroud",
        ChannelEntry {
            twitch: many(&["shroud", "shroud"]),
            ..Default::default()
        },
    )]);
    assert!(error.to_string().contains("Duplicate platform binding"));
}

#[test]
fn throws_on_a_duplicate_across_a_string_entry_and_another_entrys_array() {
    let error = duplicate(vec![
        (
            "Shroud",
            ChannelEntry {
                twitch: one("shroud"),
                ..Default::default()
            },
        ),
        (
            "OtherName",
            ChannelEntry {
                twitch: many(&["shroud", "other"]),
                ..Default::default()
            },
        ),
    ]);
    assert!(error.to_string().contains("Duplicate platform binding"));
}

#[test]
fn throws_when_two_entries_normalize_to_the_same_display_name_id() {
    let error = duplicate(vec![
        (
            "Destiny",
            ChannelEntry {
                kick: one("destiny"),
                ..Default::default()
            },
        ),
        (
            "DESTINY",
            ChannelEntry {
                twitch: one("destinytv"),
                ..Default::default()
            },
        ),
    ]);
    assert!(error.to_string().contains("Duplicate streamer"));
}

#[test]
fn applies_pushover_token_from_the_entry() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            kick: one("destiny"),
            pushover_token: Some("tok-abc".into()),
            ..Default::default()
        },
    )]))
    .unwrap();
    assert_eq!(result[0].pushover_token.as_deref(), Some("tok-abc"));
}

#[test]
fn applies_live_notifications_from_the_entry() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            kick: one("destiny"),
            live_notifications: Some(false),
            ..Default::default()
        },
    )]))
    .unwrap();
    assert_eq!(result[0].live_notifications, Some(false));
}

#[test]
fn leaves_live_notifications_undefined_without_an_override() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            kick: one("destiny"),
            ..Default::default()
        },
    )]))
    .unwrap();
    assert_eq!(result[0].live_notifications, None);
}

#[test]
fn defaults_tier_to_primary_without_an_override() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            kick: one("destiny"),
            ..Default::default()
        },
    )]))
    .unwrap();
    assert_eq!(result[0].tier, StreamerTier::Primary);
}

#[test]
fn applies_tier_from_the_entry() {
    let result = build_streamers(&config(vec![(
        "Destiny",
        ChannelEntry {
            kick: one("destiny"),
            tier: Some(StreamerTier::Background),
            ..Default::default()
        },
    )]))
    .unwrap();
    assert_eq!(result[0].tier, StreamerTier::Background);
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
