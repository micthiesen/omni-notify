//! Port of `src/live-check/dgg.spec.ts`. The websocket cases run against an
//! in-process tungstenite server instead of a fake socket object.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::SinkExt;
use omni_api::streamers::{DggPresence, StreamerTier};
use omni_live::dgg::{
    DggEmbed, DggFeed, DggFeedSource, DggHosting, DggMediaMetadata, ResolvedDggStreams,
    SelectedDggStream, WebSocketDggFeed, resolve_dgg_streams,
};
use omni_live::streamers::{DiscoverySource, Streamer};
use omni_live::{Platform, PlatformBinding};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

struct E<'a> {
    platform: &'a str,
    id: &'a str,
    display_name: Option<&'a str>,
    count: i64,
    live: bool,
    viewers: i64,
}

impl<'a> E<'a> {
    fn new(platform: &'a str, id: &'a str) -> Self {
        Self {
            platform,
            id,
            display_name: None,
            count: 10,
            live: true,
            viewers: 735,
        }
    }
    fn count(mut self, count: i64) -> Self {
        self.count = count;
        self
    }
    fn name(mut self, name: &'a str) -> Self {
        self.display_name = Some(name);
        self
    }
    fn viewers(mut self, viewers: i64) -> Self {
        self.viewers = viewers;
        self
    }
    fn offline(mut self) -> Self {
        self.live = false;
        self
    }
    fn build(self) -> DggEmbed {
        let display_name = self.display_name.unwrap_or(self.id).to_owned();
        DggEmbed {
            platform: self.platform.into(),
            id: self.id.into(),
            count: self.count,
            media_platform: self.platform.into(),
            media_id: self.id.into(),
            metadata: DggMediaMetadata {
                preview_url: Some(format!("https://images.test/{}.jpg", self.id)),
                title: Some(format!("{display_name}'s title")),
                display_name,
                created_date: Some("2026-08-22T20:24:55+00:00".into()),
                live: self.live,
                viewers: Some(self.viewers),
            },
        }
    }
}

fn hosting(platform: &str, id: &str, name: &str) -> DggHosting {
    DggHosting {
        id: id.into(),
        display_name: name.into(),
        platform: platform.into(),
        title: None,
        preview: None,
    }
}

fn all() -> HashSet<Platform> {
    Platform::ALL.into_iter().collect()
}

fn configured(
    id: &str,
    name: &str,
    platform: Platform,
    username: &str,
    tier: StreamerTier,
) -> Streamer {
    Streamer::new(
        id,
        name,
        vec![PlatformBinding::new(platform, username)],
        tier,
    )
}

fn resolve(
    feed: &DggFeed,
    limit: usize,
    streamers: &[Streamer],
    aliases: &[(&str, &str)],
) -> ResolvedDggStreams {
    let aliases: HashMap<String, String> = aliases
        .iter()
        .map(|(a, b)| ((*a).into(), (*b).into()))
        .collect();
    resolve_dgg_streams(feed, limit, streamers, &all(), &aliases)
}

fn ids(selected: &[SelectedDggStream]) -> Vec<&str> {
    selected.iter().map(|s| s.streamer.id.as_str()).collect()
}

fn feed(embeds: Vec<DggEmbed>, hosting: Option<DggHosting>, destiny_live: bool) -> DggFeed {
    DggFeed {
        embeds,
        hosting,
        destiny_live,
    }
}

#[test]
fn selects_nothing_when_the_feature_is_disabled() {
    let resolved = resolve(
        &feed(vec![E::new("kick", "ignored").build()], None, false),
        0,
        &[],
        &[],
    );
    assert!(resolved.discovered.is_empty());
}

#[test]
fn uses_the_only_slot_for_an_active_host_when_it_has_the_most_dgg_viewers() {
    let resolved = resolve(
        &feed(
            vec![
                E::new("twitch", "host").count(1_000).build(),
                E::new("kick", "top").count(999).build(),
            ],
            Some(hosting("twitch", "host", "Host")),
            false,
        ),
        1,
        &[],
        &[],
    );
    assert_eq!(ids(&resolved.discovered), ["dgg:twitch:host"]);
    assert!(resolved.discovered[0].hosted);
}

#[test]
fn selects_the_top_n_discoveries_by_dgg_viewers_without_host_priority() {
    let resolved = resolve(
        &feed(
            vec![
                E::new("twitch", "small-host")
                    .count(5)
                    .viewers(1_000_000)
                    .build(),
                E::new("kick", "popular").count(100).viewers(1).build(),
                E::new("youtube", "second").count(80).viewers(2).build(),
            ],
            Some(hosting("twitch", "small-host", "Small Host")),
            false,
        ),
        2,
        &[],
        &[],
    );
    assert_eq!(
        ids(&resolved.discovered),
        ["dgg:kick:popular", "dgg:youtube:second"]
    );
    let viewers: Vec<Option<i64>> = resolved
        .discovered
        .iter()
        .map(|s| s.streamer.dgg.and_then(|d| d.viewers))
        .collect();
    assert_eq!(viewers, [Some(100), Some(80)]);
}

#[test]
fn deduplicates_sources_and_fills_past_configured_high_ranked_embeds() {
    let resolved = resolve(
        &feed(
            vec![
                E::new("twitch", "configured").count(500).build(),
                E::new("kick", "duplicate").count(400).build(),
                E::new("kick", "duplicate").count(300).build(),
                E::new("youtube", "filled").count(200).build(),
            ],
            None,
            false,
        ),
        2,
        &[configured(
            "configured",
            "Configured",
            Platform::Twitch,
            "configured",
            StreamerTier::Primary,
        )],
        &[],
    );
    assert_eq!(
        ids(&resolved.discovered),
        ["dgg:kick:duplicate", "dgg:youtube:filled"]
    );
}

#[test]
fn merges_hosted_and_viewer_metadata_onto_a_configured_streamer() {
    let foo = configured(
        "foo",
        "Foo",
        Platform::Kick,
        "foo-on-kick",
        StreamerTier::Primary,
    );
    let resolved = resolve(
        &feed(
            vec![
                E::new("twitch", "foo-live").name("Foo").count(81).build(),
                E::new("kick", "discovered").count(70).build(),
                E::new("kick", "foo-on-kick").name("Foo").count(19).build(),
            ],
            Some(hosting("twitch", "foo-live", "foo")),
            false,
        ),
        1,
        std::slice::from_ref(&foo),
        &[],
    );
    assert_eq!(
        resolved.configured_presence.get("foo"),
        Some(&DggPresence {
            hosted: true,
            viewers: Some(100)
        })
    );
    assert_eq!(ids(&resolved.discovered), ["dgg:kick:discovered"]);
    assert_eq!(
        foo,
        configured(
            "foo",
            "Foo",
            Platform::Kick,
            "foo-on-kick",
            StreamerTier::Primary
        )
    );
}

#[test]
fn enriches_configured_sources_after_the_discovery_limit_is_full() {
    let resolved = resolve(
        &feed(
            vec![
                E::new("kick", "first").count(100).build(),
                E::new("youtube", "video-id")
                    .name("Configured Late")
                    .count(5)
                    .build(),
            ],
            None,
            false,
        ),
        1,
        &[configured(
            "configured-late",
            "Configured Late",
            Platform::YouTube,
            "@configured",
            StreamerTier::Background,
        )],
        &[("youtube:video-id", "youtube:@configured")],
    );
    assert_eq!(ids(&resolved.discovered), ["dgg:kick:first"]);
    assert_eq!(
        resolved.configured_presence.get("configured-late"),
        Some(&DggPresence {
            hosted: false,
            viewers: Some(5)
        })
    );
}

#[test]
fn merges_a_verified_youtube_owner_regardless_of_display_name() {
    for display_name in ["imreallyimportant", "@ImReallyImportant"] {
        let resolved = resolve(
            &feed(
                vec![
                    E::new("youtube", "p0oUwXqr0ds")
                        .name(display_name)
                        .count(24)
                        .build(),
                    E::new("kick", "imreallyimportant").count(34).build(),
                    E::new("twitch", "another-streamer").count(5).build(),
                ],
                None,
                false,
            ),
            1,
            &[configured(
                "iri",
                "IRI",
                Platform::YouTube,
                "@imreallyimportant",
                StreamerTier::Background,
            )],
            &[
                ("kick:imreallyimportant", "youtube:@imreallyimportant"),
                ("youtube:p0oUwXqr0ds", "youtube:@imreallyimportant"),
            ],
        );
        assert_eq!(ids(&resolved.discovered), ["dgg:twitch:another-streamer"]);
        assert_eq!(
            resolved.configured_presence.get("iri"),
            Some(&DggPresence {
                hosted: false,
                viewers: Some(58)
            })
        );
        let platforms: Vec<Platform> = resolved.configured_sources["iri"]
            .iter()
            .map(|entry| entry.streamer.bindings[0].platform)
            .collect();
        assert_eq!(platforms, [Platform::Kick, Platform::YouTube]);
    }
}

#[test]
fn does_not_guess_youtube_ownership_from_spaced_names() {
    for ambiguous in [false, true] {
        let mut streamers = vec![configured(
            "whick",
            "Whick",
            Platform::YouTube,
            "@Whick-TV",
            StreamerTier::Background,
        )];
        if ambiguous {
            streamers.push(configured(
                "other",
                "Whick TV",
                Platform::YouTube,
                "@other",
                StreamerTier::Background,
            ));
        }
        let resolved = resolve(
            &feed(
                vec![
                    E::new("youtube", "vcTFGmR6Yns")
                        .name("Whick TV")
                        .count(50)
                        .build(),
                ],
                None,
                false,
            ),
            1,
            &streamers,
            &[],
        );
        assert_eq!(resolved.discovered.len(), 1);
        assert_eq!(resolved.configured_presence.get("whick"), None);
        assert!(resolved.configured_sources.is_empty());
    }
}

#[test]
fn does_not_match_a_youtube_handle_across_platforms_or_an_ambiguous_name() {
    for platform in [Platform::Kick, Platform::YouTube] {
        let mut streamers = vec![configured(
            "iri",
            "IRI",
            Platform::YouTube,
            "@imreallyimportant",
            StreamerTier::Background,
        )];
        if platform == Platform::YouTube {
            streamers.push(configured(
                "other",
                "imreallyimportant",
                Platform::YouTube,
                "@other",
                StreamerTier::Background,
            ));
        }
        let resolved = resolve(
            &feed(
                vec![
                    E::new(platform.as_str(), "video-id")
                        .name("imreallyimportant")
                        .build(),
                ],
                None,
                false,
            ),
            1,
            &streamers,
            &[],
        );
        assert_eq!(resolved.discovered.len(), 1);
        assert!(resolved.configured_presence.is_empty());
    }
}

#[test]
fn attaches_a_profile_linked_platform_source_to_its_configured_identity() {
    let resolved = resolve(
        &feed(
            vec![
                E::new("kick", "imreallyimportant")
                    .name("imreallyimportant")
                    .viewers(475)
                    .build(),
            ],
            None,
            false,
        ),
        1,
        &[configured(
            "iri",
            "IRI",
            Platform::YouTube,
            "@imreallyimportant",
            StreamerTier::Background,
        )],
        &[("kick:imreallyimportant", "youtube:@imreallyimportant")],
    );
    assert!(resolved.discovered.is_empty());
    let entry = &resolved.configured_sources["iri"][0];
    assert_eq!(entry.streamer.bindings[0].platform, Platform::Kick);
    assert_eq!(entry.streamer.bindings[0].username, "imreallyimportant");
    assert_eq!(entry.status.viewer_count, Some(475));
}

#[test]
fn ranks_a_host_by_dgg_viewers_and_retains_its_metadata() {
    let host = DggHosting {
        title: Some("A hosted stream".into()),
        preview: Some("https://images.test/host.jpg".into()),
        ..hosting("twitch", "host_channel", "Host Channel")
    };
    let resolved = resolve(
        &feed(
            vec![
                E::new("kick", "first").count(99).build(),
                E::new("twitch", "host_channel").count(5).build(),
                E::new("twitch", "host_channel").count(50).build(),
                E::new("youtube", "video-id").count(40).build(),
            ],
            Some(host),
            false,
        ),
        3,
        &[],
        &[],
    );
    let selected = &resolved.discovered;
    assert_eq!(
        ids(selected),
        [
            "dgg:kick:first",
            "dgg:twitch:host_channel",
            "dgg:youtube:video-id"
        ]
    );
    let hosted = &selected[1];
    assert!(hosted.hosted);
    assert_eq!(
        hosted.preview_url.as_deref(),
        Some("https://images.test/host.jpg")
    );
    assert_eq!(hosted.url, "https://www.twitch.tv/host_channel");
    assert_eq!(hosted.status.title, "A hosted stream");
    assert_eq!(hosted.status.viewer_count, Some(735));
    assert_eq!(
        hosted.status.started_at.as_deref(),
        Some("2026-08-22T20:24:55+00:00")
    );
    assert_eq!(hosted.streamer.discovery_source, Some(DiscoverySource::Dgg));
    assert_eq!(
        hosted.streamer.dgg,
        Some(DggPresence {
            hosted: true,
            viewers: Some(50)
        })
    );
    assert_eq!(
        hosted.streamer.bindings[0].url_override.as_deref(),
        Some("https://www.twitch.tv/host_channel")
    );
    let first = &selected[0];
    assert!(!first.hosted);
    assert_eq!(first.embed_count, Some(99));
    assert_eq!(first.status.title, "first's title");
    assert_eq!(first.streamer.tier, StreamerTier::Background);
    assert_eq!(
        first.streamer.dgg,
        Some(DggPresence {
            hosted: false,
            viewers: Some(99)
        })
    );
    assert_eq!(selected[2].url, "https://www.youtube.com/watch?v=video-id");
}

#[test]
fn filters_configured_collisions_unusable_platforms_and_non_live_embeds() {
    let streamers = [
        configured(
            "known-binding",
            "Someone Else",
            Platform::Twitch,
            "KnownChannel",
            StreamerTier::Primary,
        ),
        configured(
            "same-name",
            "Display Collision",
            Platform::Kick,
            "different",
            StreamerTier::Primary,
        ),
    ];
    let embeds = vec![
        E::new("twitch", "knownchannel").build(),
        E::new("kick", "new-id").name("display collision").build(),
        E::new("youtube", "unavailable").build(),
        E::new("rumble", "unsupported").build(),
        E::new("kick", "offline").offline().build(),
        E::new("kick", "kept").build(),
    ];
    let available: HashSet<Platform> = [Platform::Twitch, Platform::Kick].into_iter().collect();
    let resolved = resolve_dgg_streams(
        &feed(embeds, None, false),
        10,
        &streamers,
        &available,
        &HashMap::new(),
    );
    assert_eq!(ids(&resolved.discovered), ["dgg:kick:kept"]);
}

#[test]
fn suppresses_hosting_when_destiny_is_live() {
    let resolved = resolve(
        &feed(
            vec![E::new("kick", "still-embedded").build()],
            Some(hosting("twitch", "stale-host", "Stale Host")),
            true,
        ),
        1,
        &[],
        &[],
    );
    assert_eq!(ids(&resolved.discovered), ["dgg:kick:still-embedded"]);
    assert!(!resolved.discovered[0].hosted);
}

// --- websocket snapshot -------------------------------------------------------

/// Serves one websocket connection, sending `frames` and then holding it open.
async fn serve(frames: Vec<String>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        for frame in frames {
            if socket.send(Message::text(frame)).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    format!("ws://{address}")
}

fn embed_json(platform: &str, id: &str) -> serde_json::Value {
    json!({
        "platform": platform, "id": id, "count": 10,
        "mediaItem": {
            "identifier": {"platform": platform, "mediaId": id},
            "metadata": {"displayName": id, "title": format!("{id}'s title"), "live": true, "viewers": 735}
        }
    })
}

#[tokio::test]
async fn rejects_when_a_complete_snapshot_does_not_arrive_before_the_timeout() {
    let url = serve(vec![]).await;
    let error = WebSocketDggFeed::new(url, Duration::from_millis(200))
        .fetch()
        .await
        .unwrap_err();
    assert_eq!(error.message, "Timed out waiting for DGG live snapshot");
}

#[tokio::test]
async fn waits_for_the_complete_snapshot_in_any_order_and_suppresses_stale_hosting() {
    let url = serve(vec![
        json!({"type": "dggApi:hosting", "data": {"platform": "twitch", "id": "host", "displayName": "Host", "title": "Hosted title", "preview": null}}).to_string(),
        json!({"type": "unrelated", "data": {}}).to_string(),
        json!({"type": "dggApi:embeds", "data": [embed_json("kick", "x")]}).to_string(),
        json!({"type": "dggApi:streamInfo", "data": {"streams": {"twitch": null, "youtube": {"live": true, "extra": "allowed"}}}}).to_string(),
    ])
    .await;
    let feed = WebSocketDggFeed::new(url, Duration::from_secs(5))
        .fetch()
        .await
        .unwrap();
    assert!(feed.destiny_live);
    assert_eq!(feed.hosting, None);
    assert_eq!(feed.embeds.len(), 1);
    assert_eq!(feed.embeds[0].id, "x");
}

#[tokio::test]
async fn rejects_malformed_relevant_payloads_instead_of_returning_a_partial_feed() {
    let url = serve(vec![
        json!({"type": "dggApi:embeds", "data": [{"nope": true}]}).to_string(),
    ])
    .await;
    let error = WebSocketDggFeed::new(url, Duration::from_secs(5))
        .fetch()
        .await
        .unwrap_err();
    assert!(
        error.message.starts_with("Invalid dggApi:embeds payload"),
        "{error}"
    );
}

#[tokio::test]
async fn reports_a_refused_connection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = WebSocketDggFeed::new(format!("ws://{address}"), Duration::from_secs(5))
        .fetch()
        .await
        .unwrap_err();
    assert_eq!(error.message, "DGG websocket connection failed");
}

#[test]
fn rejects_a_relevant_envelope_without_a_payload() {
    use omni_live::dgg::SnapshotAssembler;
    // `null` is a valid "nobody hosted" payload; an absent payload is not.
    let mut assembler = SnapshotAssembler::default();
    assert_eq!(
        assembler.accept(r#"{"type":"dggApi:hosting","data":null}"#),
        Ok(None)
    );
    let error = SnapshotAssembler::default()
        .accept(r#"{"type":"dggApi:hosting"}"#)
        .unwrap_err();
    assert_eq!(error.message, "Invalid dggApi:hosting payload: Required");
}
