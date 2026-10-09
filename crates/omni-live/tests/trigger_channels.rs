//! Trigger channels.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_api::streamers::{StreamerTier, TriggerChannel, TriggerChannelType};
use omni_live::streamers::Streamer;
use omni_live::trigger_channels::to_trigger_channels;
use omni_live::{Platform, PlatformBinding};

fn streamer(display_name: &str, bindings: Vec<PlatformBinding>) -> Streamer {
    Streamer::new(
        display_name.to_lowercase(),
        display_name,
        bindings,
        StreamerTier::Primary,
    )
}

#[test]
fn maps_a_youtube_binding_to_a_channel_with_the_live_page_url() {
    let channels = to_trigger_channels(&[streamer(
        "Destiny",
        vec![PlatformBinding::new(Platform::YouTube, "@destiny")],
    )]);
    assert_eq!(
        channels,
        vec![TriggerChannel {
            key: "destiny".into(),
            display_name: "Destiny".into(),
            kind: TriggerChannelType::Youtube,
            url: Some("https://www.youtube.com/@destiny/live".into()),
        }]
    );
}

#[test]
fn maps_a_twitch_binding_without_a_url() {
    let channels = to_trigger_channels(&[streamer(
        "Jerma",
        vec![PlatformBinding::new(Platform::Twitch, "jerma985")],
    )]);
    assert_eq!(
        channels,
        vec![TriggerChannel {
            key: "jerma".into(),
            display_name: "Jerma".into(),
            kind: TriggerChannelType::Twitch,
            url: None,
        }]
    );
}

#[test]
fn prefers_youtube_over_twitch_for_multi_platform_streamers() {
    let channels = to_trigger_channels(&[streamer(
        "Both",
        vec![
            PlatformBinding::new(Platform::Twitch, "both"),
            PlatformBinding::new(Platform::YouTube, "@both"),
        ],
    )]);
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].kind, TriggerChannelType::Youtube);
}

#[test]
fn prefers_twitch_over_kick_for_multi_platform_streamers() {
    let channels = to_trigger_channels(&[streamer(
        "Mixed",
        vec![
            PlatformBinding::new(Platform::Kick, "mixed"),
            PlatformBinding::new(Platform::Twitch, "mixed"),
        ],
    )]);
    assert_eq!(channels[0].kind, TriggerChannelType::Twitch);
}

#[test]
fn maps_a_kick_binding_to_the_channels_universal_link() {
    let channels = to_trigger_channels(&[streamer(
        "KickOnly",
        vec![PlatformBinding::new(Platform::Kick, "kickonly")],
    )]);
    assert_eq!(
        channels,
        vec![TriggerChannel {
            key: "kickonly".into(),
            display_name: "KickOnly".into(),
            kind: TriggerChannelType::Kick,
            url: Some("https://kick.com/kickonly".into()),
        }]
    );
}
