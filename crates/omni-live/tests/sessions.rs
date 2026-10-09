//! Port of `src/live-check/sessions.spec.ts`. The ISO-string case decodes a
//! status whose `startedAt` is a string (the typed `JsDate` accepts it).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_live::sessions::{
    MAX_SESSION_AGE_MS, MAX_SESSIONS, StreamSession, StreamSessions, append_session,
    session_from_live_status,
};
use omni_live::status::{LiveStatus, StreamerStatus};
use omni_live::{Platform, PlatformBinding};
use omni_store::cbor::{self, Extra, JsValue};

const HOUR: i64 = 60 * 60 * 1000;

fn session(started_at: i64, ended_at: i64) -> StreamSession {
    StreamSession {
        started_at,
        ended_at,
        duration_ms: HOUR,
        peak_viewers: 123,
        title: "Test stream".into(),
        platform: Platform::Twitch,
        username: "tester".into(),
    }
}

fn base() -> LiveStatus {
    LiveStatus {
        streamer_id: "tester".into(),
        primary: PlatformBinding::new(Platform::YouTube, "@tester"),
        primary_title: "Big stream".into(),
        started_at: 1_000_000,
        max_viewer_count: 456,
        viewer_count: None,
        sources: None,
        category: None,
        extra: Extra::new(),
    }
}

#[test]
fn builds_a_completed_session_from_live_status() {
    assert_eq!(
        session_from_live_status(&base(), 1_000_000 + HOUR),
        StreamSession {
            started_at: 1_000_000,
            ended_at: 1_000_000 + HOUR,
            duration_ms: HOUR,
            peak_viewers: 456,
            title: "Big stream".into(),
            platform: Platform::YouTube,
            username: "@tester".into(),
        }
    );
}

#[test]
fn handles_started_at_as_an_iso_string_json_round_trip() {
    let value = JsValue::Object(
        [
            ("streamerId", JsValue::String("tester".into())),
            ("isLive", JsValue::Bool(true)),
            (
                "primary",
                JsValue::Object(
                    [
                        ("platform".to_owned(), JsValue::String("youtube".into())),
                        ("username".to_owned(), JsValue::String("@tester".into())),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ),
            ("primaryTitle", JsValue::String("Big stream".into())),
            (
                "startedAt",
                JsValue::String("1970-01-01T00:16:40.000Z".into()),
            ),
            ("maxViewerCount", JsValue::Int(456)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect(),
    );
    let status: StreamerStatus = cbor::from_value(value).unwrap();
    let live = status.as_live().unwrap();
    let session = session_from_live_status(live, 1_000_000 + HOUR);
    assert_eq!(session.started_at, 1_000_000);
    assert_eq!(session.duration_ms, HOUR);
}

#[test]
fn clamps_negative_durations_to_zero() {
    assert_eq!(session_from_live_status(&base(), 500_000).duration_ms, 0);
}

fn empty() -> StreamSessions {
    StreamSessions::empty("tester")
}

#[test]
fn appends_to_an_empty_list() {
    assert_eq!(
        append_session(empty(), session(1_000_000, 1_000_000 + HOUR))
            .sessions
            .len(),
        1
    );
}

#[test]
fn keeps_sessions_ordered_oldest_to_newest() {
    let first = session(1_000_000, 1_000_000 + HOUR);
    let second = session(2_000_000, 2_000_000 + HOUR);
    let result = append_session(append_session(empty(), first), second);
    let ends: Vec<i64> = result.sessions.iter().map(|s| s.ended_at).collect();
    assert_eq!(ends, [1_000_000 + HOUR, 2_000_000 + HOUR]);
}

#[test]
fn prunes_sessions_older_than_the_age_cap() {
    let now = MAX_SESSION_AGE_MS + 10 * HOUR;
    let stale = session(0, HOUR);
    let fresh = session(now - HOUR, now);
    let mut data = empty();
    data.sessions.push(stale);
    assert_eq!(append_session(data, fresh.clone()).sessions, vec![fresh]);
}

#[test]
fn caps_the_list_at_max_sessions() {
    let mut data = empty();
    data.sessions = (0..MAX_SESSIONS as i64)
        .map(|i| session(i * HOUR, i * HOUR + HOUR))
        .collect();
    let newest = session(MAX_SESSIONS as i64 * HOUR, (MAX_SESSIONS as i64 + 1) * HOUR);
    let result = append_session(data, newest.clone());
    assert_eq!(result.sessions.len(), MAX_SESSIONS);
    assert_eq!(result.sessions.last(), Some(&newest));
    assert_eq!(result.sessions[0].started_at, HOUR);
}
