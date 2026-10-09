//! Port of `src/live-check/channelsConfig.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use omni_api::streamers::StreamerTier;
use omni_live::channels::{
    ChannelEntry, ChannelsConfig, ChannelsConfigError, LiveCheckConfig, Usernames,
    load_channels_config,
};

struct Dir(tempfile::TempDir);

impl Dir {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn with_config(&self, content: &str) -> PathBuf {
        let path = self.0.path().join("channels.json");
        std::fs::write(&path, content).unwrap();
        path
    }
}

fn one(name: &str) -> Option<Usernames> {
    Some(Usernames::One(name.to_owned()))
}

fn config(entries: Vec<(&str, ChannelEntry)>, dgg_top_embeds: u32) -> LiveCheckConfig {
    LiveCheckConfig {
        channels: entries
            .into_iter()
            .map(|(name, entry)| (name.to_owned(), entry))
            .collect::<ChannelsConfig>(),
        dgg_top_embeds,
    }
}

fn load_err(content: &str) -> String {
    let dir = Dir::new();
    match load_channels_config(&dir.with_config(content)) {
        Ok(config) => panic!("expected an error, got {config:?}"),
        Err(error) => error.to_string(),
    }
}

fn load(content: &str) -> LiveCheckConfig {
    let dir = Dir::new();
    load_channels_config(&dir.with_config(content)).unwrap()
}

#[test]
fn returns_empty_when_the_file_does_not_exist() {
    let dir = Dir::new();
    let loaded = load_channels_config(&dir.0.path().join("does-not-exist.json")).unwrap();
    assert_eq!(loaded, LiveCheckConfig::default());
}

#[test]
fn parses_a_full_production_shaped_config() {
    let loaded = load(
        r#"{"Destiny":{"youtube":"@destiny","kick":"destiny","pushoverToken":"tok"},
            "Hutch":{"youtube":"@hutch","liveNotifications":false},
            "Jerma":{"twitch":"jerma985","liveNotifications":false}}"#,
    );
    assert_eq!(
        loaded,
        config(
            vec![
                (
                    "Destiny",
                    ChannelEntry {
                        youtube: one("@destiny"),
                        kick: one("destiny"),
                        pushover_token: Some("tok".into()),
                        ..Default::default()
                    }
                ),
                (
                    "Hutch",
                    ChannelEntry {
                        youtube: one("@hutch"),
                        live_notifications: Some(false),
                        ..Default::default()
                    }
                ),
                (
                    "Jerma",
                    ChannelEntry {
                        twitch: one("jerma985"),
                        live_notifications: Some(false),
                        ..Default::default()
                    }
                ),
            ],
            0
        )
    );
    let names: Vec<&String> = loaded.channels.keys().collect();
    assert_eq!(names, ["Destiny", "Hutch", "Jerma"]);
}

#[test]
fn accepts_a_string_array_of_usernames_on_a_platform_field() {
    let loaded = load(r#"{"Destiny":{"twitch":["destiny","destinyalt"]}}"#);
    assert_eq!(
        loaded.channels["Destiny"].twitch,
        Some(Usernames::Many(vec!["destiny".into(), "destinyalt".into()]))
    );
}

#[test]
fn accepts_an_entry_with_a_tier_field() {
    let loaded = load(r#"{"Destiny":{"kick":"destiny","tier":"background"}}"#);
    assert_eq!(
        loaded.channels["Destiny"].tier,
        Some(StreamerTier::Background)
    );
}

#[test]
fn throws_on_malformed_json() {
    let dir = Dir::new();
    let error = load_channels_config(&dir.with_config("{ not valid json")).unwrap_err();
    assert!(matches!(error, ChannelsConfigError::Parse { .. }));
    assert!(
        error
            .to_string()
            .contains("Failed to parse channels config")
    );
}

#[test]
fn throws_on_a_schema_violation_bad_tier_enum_value() {
    assert!(
        load_err(r#"{"Destiny":{"kick":"destiny","tier":"vip"}}"#)
            .contains("Invalid channels config")
    );
}

#[test]
fn rejects_an_unknown_key_on_an_entry() {
    assert!(
        load_err(r#"{"Destiny":{"kick":"destiny","twich":"typo"}}"#)
            .contains("Invalid channels config")
    );
}

#[test]
fn rejects_an_entry_with_no_platform_fields() {
    assert!(load_err(r#"{"Destiny":{"pushoverToken":"tok"}}"#).contains("at least one platform"));
}

#[test]
fn rejects_an_entry_with_an_empty_username_string() {
    assert!(load_err(r#"{"Destiny":{"kick":""}}"#).contains("Invalid channels config"));
}

#[test]
fn rejects_an_entry_with_an_empty_usernames_array() {
    assert!(load_err(r#"{"Destiny":{"twitch":[]}}"#).contains("Invalid channels config"));
}

#[test]
fn rejects_a_blank_display_name_key() {
    assert!(load_err(r#"{"  ":{"twitch":"someone"}}"#).contains("display name must not be blank"));
}

#[test]
fn throws_when_tier_background_is_combined_with_live_notifications_true() {
    assert!(
        load_err(r#"{"Destiny":{"kick":"destiny","tier":"background","liveNotifications":true}}"#)
            .contains("contradicts the background tier")
    );
}

#[test]
fn throws_when_tier_background_is_combined_with_live_notifications_false() {
    assert!(
        load_err(r#"{"Destiny":{"kick":"destiny","tier":"background","liveNotifications":false}}"#)
            .contains("redundant")
    );
}

#[test]
fn allows_live_notifications_false_without_a_tier() {
    let loaded = load(r#"{"Destiny":{"kick":"destiny","liveNotifications":false}}"#);
    assert_eq!(loaded.channels["Destiny"].live_notifications, Some(false));
}

#[test]
fn allows_tier_primary_alongside_an_explicit_live_notifications() {
    let loaded =
        load(r#"{"Destiny":{"kick":"destiny","tier":"primary","liveNotifications":false}}"#);
    assert_eq!(loaded.channels["Destiny"].tier, Some(StreamerTier::Primary));
    assert_eq!(loaded.channels["Destiny"].live_notifications, Some(false));
}

#[test]
fn accepts_a_destiny_gg_top_embeds_count_alongside_channels() {
    let loaded = load(r#"{"dggTopEmbeds":3,"Destiny":{"kick":"destiny"}}"#);
    assert_eq!(loaded.dgg_top_embeds, 3);
    assert_eq!(loaded.channels.len(), 1);
}

#[test]
fn rejects_an_invalid_destiny_gg_top_embeds_count() {
    assert!(load_err(r#"{"dggTopEmbeds":-1}"#).contains("dggTopEmbeds"));
    assert!(load_err(r#"{"dggTopEmbeds":21}"#).contains("dggTopEmbeds"));
    assert!(load_err(r#"{"dggTopEmbeds":1.5}"#).contains("dggTopEmbeds"));
}

#[test]
fn rejects_a_non_object_root_and_null_tokens() {
    assert!(load_err("[]").contains("expected a JSON object"));
    assert!(
        load_err(r#"{"Destiny":{"kick":"destiny","pushoverToken":null}}"#)
            .contains("pushoverToken")
    );
}
