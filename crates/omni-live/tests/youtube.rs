//! Port of `src/live-check/platforms/youtube.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_live::platform::{FetchedLive, FetchedStatus};
use omni_live::platforms::extract_youtube_status;
use serde_json::{Value, json};

fn wrap(player_response: &Value, html: &str) -> String {
    format!("<script>var ytInitialPlayerResponse = {player_response};</script>{html}")
}

fn live_player_response() -> Value {
    json!({
        "videoDetails": {"isLive": true, "isLiveContent": true},
        "microformat": {"playerMicroformatRenderer": {"liveBroadcastDetails": {"isLiveNow": true}}}
    })
}

fn live(title: &str, viewers: Option<i64>) -> FetchedStatus {
    FetchedStatus::Live(FetchedLive {
        title: title.into(),
        viewer_count: viewers,
        category: None,
        started_at: None,
    })
}

#[test]
fn extracts_the_title_and_viewer_count_for_an_active_livestream() {
    let html = wrap(
        &live_player_response(),
        r#"<meta name="title" content="Drum &amp; Bass Non-Stop Liquid"><script>var liveData = {"viewCount":{"runs":[{"text":"12,345"}]}};</script>"#,
    );
    assert_eq!(
        extract_youtube_status(&html),
        live("Drum & Bass Non-Stop Liquid", Some(12345))
    );
}

#[test]
fn returns_offline_when_the_player_response_is_not_live() {
    let html = wrap(
        &json!({"videoDetails": {"isLive": false, "isLiveContent": false}}),
        "",
    );
    assert_eq!(extract_youtube_status(&html), FetchedStatus::Offline);
}

#[test]
fn returns_offline_for_a_normal_channel_page_with_no_player_response() {
    let html = [
        r#"<script>var ytInitialData = {"contents":{}};</script>"#,
        "<script>loadInitialData(a.ytInitialData,a.ytInitialPlayerResponse);</script>",
        r#"<script>var unrelatedData = {"isLive":true,"isLiveNow":true};</script>"#,
    ]
    .join("");
    assert_eq!(extract_youtube_status(&html), FetchedStatus::Offline);
}

#[test]
fn ignores_scheduled_streams_and_unrelated_live_markers() {
    let scheduled = json!({
        "videoDetails": {"title": "LibCon", "isLive": true, "isLiveContent": true},
        "microformat": {"playerMicroformatRenderer": {"liveBroadcastDetails": {
            "isLiveNow": false, "startTimestamp": "2026-08-09T18:00:00Z"
        }}}
    });
    let html = wrap(
        &scheduled,
        r#"<script>var unrelatedData = {"isLive":true,"isLiveNow":true};</script>"#,
    );
    assert_eq!(extract_youtube_status(&html), FetchedStatus::Offline);
}

#[test]
fn uses_the_active_player_response_when_unrelated_scheduled_data_also_exists() {
    let html = wrap(
        &live_player_response(),
        &[
            r#"<script>var scheduledEvent = {"title":"LibCon","isLive":true,"liveBroadcastDetails":{"isLiveNow":false}};</script>"#,
            r#"<meta name="title" content="Whick Is Actually Live &amp; Streaming">"#,
            r#"<script>var liveData = {"viewCount":{"runs":[{"text":"4,321"}]}};</script>"#,
        ]
        .join(""),
    );
    assert_eq!(
        extract_youtube_status(&html),
        live("Whick Is Actually Live & Streaming", Some(4321))
    );
}

#[test]
fn returns_unknown_when_yt_initial_player_response_is_missing() {
    assert_eq!(
        extract_youtube_status("<div>Some other content</div>"),
        FetchedStatus::unknown("Response missing expected YouTube data structure")
    );
}

#[test]
fn skips_a_malformed_assignment_when_a_later_player_response_is_valid() {
    let html = [
        r#"<script>var ytInitialPlayerResponse = {"videoDetails":undefined};</script>"#.to_owned(),
        wrap(
            &live_player_response(),
            r#"<meta name="title" content="Recovered Live Stream">"#,
        ),
    ]
    .join("");
    assert_eq!(
        extract_youtube_status(&html),
        live("Recovered Live Stream", None)
    );
}

#[test]
fn returns_unknown_when_a_live_stream_has_no_title_meta_tag() {
    assert_eq!(
        extract_youtube_status(&wrap(&live_player_response(), "")),
        FetchedStatus::unknown("Live detected but failed to extract title")
    );
}

#[test]
fn extracts_the_title_when_the_page_has_multiple_meta_tags() {
    let html = wrap(
        &live_player_response(),
        r#"<meta name="description" content="Some description"><meta name="title" content="A &quot;quoted&quot; title &amp; special characters"><meta name="keywords" content="music, chill, relax">"#,
    );
    assert_eq!(
        extract_youtube_status(&html),
        live(r#"A "quoted" title & special characters"#, None)
    );
}
