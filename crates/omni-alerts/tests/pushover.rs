//! Pushover client against a local mock of the messages endpoint.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_alerts::{PushOutcome, Pushover, PushoverChannel, PushoverMessage, RecordedPush};
use omni_http::{HttpClient, HttpConfig, HttpOverrides, SideEffectMode, Url};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http(server: &MockServer) -> HttpClient {
    HttpClient::new(HttpConfig {
        connect_timeout: None,
        offline: true,
    })
    .unwrap()
    .with_overrides(HttpOverrides {
        rewrites: vec![(
            Url::parse("https://api.pushover.net").unwrap(),
            Url::parse(&server.uri()).unwrap(),
        )],
    })
}

fn pushover(server: &MockServer, mode: SideEffectMode) -> Pushover {
    Pushover::with_credentials(
        http(server),
        Some("user-key".to_owned()),
        [
            (PushoverChannel::General, "general-token".to_owned()),
            (PushoverChannel::Live, "live-token".to_owned()),
        ],
        mode,
    )
}

#[tokio::test]
async fn posts_the_form_fields_mitools_sends() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/messages.json"))
        .and(body_string_contains("token=live-token"))
        .and(body_string_contains("user=user-key"))
        .and(body_string_contains("message=Streamer+is+live"))
        .and(body_string_contains("title=Live"))
        .and(body_string_contains("url_title=Watch"))
        .and(body_string_contains("priority=-1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"status":1}"#))
        .expect(1)
        .mount(&server)
        .await;
    let outcome = pushover(&server, SideEffectMode::Live)
        .send(
            PushoverChannel::Live,
            PushoverMessage {
                message: "Streamer is live".to_owned(),
                title: Some("Live".to_owned()),
                url: Some("https://example.com/watch".to_owned()),
                url_title: Some("Watch".to_owned()),
                priority: Some(-1),
                sound: Some(String::new()),
                timestamp: Some(0),
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome, PushOutcome::Sent);
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(!body.contains("sound="), "{body}");
    assert!(!body.contains("timestamp="), "{body}");
}

#[tokio::test]
async fn a_rejection_carries_status_and_body() {
    let server = MockServer::start().await;
    Mock::given(path("/1/messages.json"))
        .respond_with(ResponseTemplate::new(400).set_body_string(r#"{"errors":["bad"]}"#))
        .mount(&server)
        .await;
    let error = pushover(&server, SideEffectMode::Live)
        .send(
            PushoverChannel::General,
            PushoverMessage {
                message: "x".to_owned(),
                ..PushoverMessage::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(400));
    assert!(error.body.contains("bad"));
    assert!(error.is_definite_rejection());
}

#[tokio::test]
async fn a_server_failure_is_not_a_definite_rejection() {
    let server = MockServer::start().await;
    Mock::given(path("/1/messages.json"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let error = pushover(&server, SideEffectMode::Live)
        .send_with_token("explicit", PushoverMessage::default())
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(503));
    assert!(!error.is_definite_rejection());
}

#[tokio::test]
async fn skips_without_a_token_or_user_and_records_in_record_mode() {
    let server = MockServer::start().await;
    let live = pushover(&server, SideEffectMode::Live);
    assert_eq!(
        live.send(PushoverChannel::Calendar, PushoverMessage::default())
            .await
            .unwrap(),
        PushOutcome::SkippedNoToken
    );
    let no_user = Pushover::with_credentials(
        http(&server),
        None,
        [(PushoverChannel::General, "t".to_owned())],
        SideEffectMode::Live,
    );
    assert_eq!(
        no_user
            .send(PushoverChannel::General, PushoverMessage::default())
            .await
            .unwrap(),
        PushOutcome::Disabled
    );
    let recording = pushover(&server, SideEffectMode::Record);
    let message = PushoverMessage {
        message: "recorded".to_owned(),
        ..PushoverMessage::default()
    };
    assert_eq!(
        recording
            .send(PushoverChannel::General, message.clone())
            .await
            .unwrap(),
        PushOutcome::Recorded
    );
    assert_eq!(
        recording.recorded(),
        vec![RecordedPush {
            token: "general-token".to_owned(),
            message,
        }]
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}
