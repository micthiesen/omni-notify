//! Show identity keys and episode ids.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_podcasts::types::{make_episode_id, make_show_id, normalize_feed_url};

#[test]
fn prefers_the_itunes_id() {
    assert_eq!(
        make_show_id(Some(123), Some("https://x.com/feed")).as_deref(),
        Some("itunes:123")
    );
}

#[test]
fn falls_back_to_the_normalized_feed_url() {
    assert_eq!(
        make_show_id(None, Some("HTTPS://Feeds.Example.com/GrayArea/")).as_deref(),
        Some("feed:feeds.example.com/grayarea")
    );
}

#[test]
fn returns_undefined_with_no_identifiers() {
    assert_eq!(make_show_id(None, None), None);
}

#[test]
fn strips_protocol_trailing_slashes_and_case() {
    assert_eq!(normalize_feed_url("http://Feeds.X.com/a/"), "feeds.x.com/a");
    assert_eq!(normalize_feed_url("https://feeds.x.com/a"), "feeds.x.com/a");
}

#[test]
fn joins_show_id_and_guid() {
    assert_eq!(make_episode_id("itunes:1", "guid-9"), "itunes:1#guid-9");
}
