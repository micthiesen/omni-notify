//! The Castro HTTP API against a local mock server. Oversized responses send
//! an over-limit body (wiremock always sets Content-Length; the streamed count
//! is covered by omni-http's bounded-read tests), and dropping a request
//! future frees its pacing permit for the next request.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use omni_core::clock::SharedClock;
use omni_http::SideEffectMode;
use omni_http::public::PublicHttpClient;
use omni_podcasts::castro::api::{CastroApi, encode_castro_query_value};
use omni_podcasts::castro::auth::CastroCredentials;
use omni_podcasts::castro::protocol::{CastroAction, CastroActionSource, CastroActionType};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const STATUS_BODY: &str = r#"{"device_status":1,"account_status":1,"latest_event_id":42}"#;

async fn api(mode: SideEffectMode, max: usize) -> (MockServer, CastroApi) {
    let server = omni_testkit::mock_server().await;
    let http = omni_testkit::mock_http(&server, &["https://tentacles.castro.fm"]);
    let clock: SharedClock = omni_testkit::test_clock(1_784_222_819_000);
    let api = CastroApi::new(
        PublicHttpClient::new(&http).allow_loopback_for_tests(),
        CastroCredentials {
            access_id: "device".into(),
            secret: b"secret".to_vec(),
        },
        clock,
        mode,
    )
    .with_max_response_bytes(max);
    (server, api)
}

#[tokio::test]
async fn does_not_retry_a_permanent_http_failure() {
    let (server, api) = api(SideEffectMode::Live, 1024).await;
    Mock::given(method("GET"))
        .and(path("/profile/sync/status"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .expect(1)
        .mount(&server)
        .await;
    let error = api.get_sync_status().await.unwrap_err();
    assert!(
        error.to_string().contains("HTTP 400: bad request"),
        "{error}"
    );
}

#[tokio::test]
async fn retries_rate_limits_and_server_failures() {
    let (server, api) = api(SideEffectMode::Live, 1024).await;
    Mock::given(method("GET"))
        .and(path("/profile/sync/status"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .expect(3)
        .mount(&server)
        .await;
    assert!(api.get_sync_status().await.is_err());
}

#[tokio::test]
async fn streams_and_decodes_a_concrete_response() {
    let (server, api) = api(SideEffectMode::Live, 1024).await;
    Mock::given(method("GET"))
        .and(path("/profile/sync/status"))
        .and(header(
            "accept",
            "application/vnd.tentacles.supertop.co+json; version=8",
        ))
        .and(header("x-tentacles-app", "castro-ios"))
        .and(header("x-tentacles-platform", "iOS"))
        .and(header(
            "x-authorization-content-sha256",
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(STATUS_BODY))
        .expect(1)
        .mount(&server)
        .await;
    let status = api.get_sync_status().await.unwrap();
    assert_eq!(status.latest_event_id, 42);
    let requests = server.received_requests().await.unwrap();
    let auth = requests[0]
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(auth.starts_with("APIAuth-HMAC-SHA256 device:"), "{auth}");
    // wiremock's header matcher splits on commas, so the Date is checked here.
    assert_eq!(
        requests[0].headers.get("date").unwrap().to_str().unwrap(),
        "Thu, 16 Jul 2026 17:26:59 GMT"
    );
}

#[tokio::test]
async fn rejects_a_fixed_length_oversized_response_before_buffering_it() {
    let (server, api) = api(SideEffectMode::Live, 8).await;
    Mock::given(method("GET"))
        .and(path("/profile/sync/status"))
        .respond_with(ResponseTemplate::new(200).set_body_string("123456789"))
        .mount(&server)
        .await;
    let error = api.get_sync_status().await.unwrap_err();
    assert!(error.to_string().contains("exceeds 8 bytes"), "{error}");
    // An oversized body is not a transient failure: one attempt only.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn cancels_a_chunked_oversized_response() {
    let (server, api) = api(SideEffectMode::Live, 8).await;
    Mock::given(method("GET"))
        .and(path("/profile/sync/status"))
        .respond_with(ResponseTemplate::new(200).set_body_string("1234567890"))
        .mount(&server)
        .await;
    let error = api.get_sync_status().await.unwrap_err();
    assert!(error.to_string().contains("exceeds 8 bytes"), "{error}");
    // An oversized body is not a transient failure: one attempt only.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn aborts_an_in_flight_request_when_dropped() {
    let (server, api) = api(SideEffectMode::Live, 1024).await;
    Mock::given(method("GET"))
        .and(path("/profile/sync/queue"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/profile/sync/status"))
        .respond_with(ResponseTemplate::new(200).set_body_string(STATUS_BODY))
        .mount(&server)
        .await;
    let api = Arc::new(api);
    // Four slow requests hold every concurrency permit until they are dropped.
    for _ in 0..4 {
        assert!(
            tokio::time::timeout(Duration::from_millis(50), api.fetch_queue())
                .await
                .is_err()
        );
    }
    let status = tokio::time::timeout(Duration::from_secs(5), api.get_sync_status())
        .await
        .expect("dropped requests released their permits")
        .unwrap();
    assert_eq!(status.latest_event_id, 42);
}

#[tokio::test]
async fn posts_signed_json_actions_once_and_records_them_in_record_mode() {
    let action = CastroAction {
        id: 1,
        episode_id: "11111111-1111-4111-8111-111111111111".into(),
        origin_event_id: "22222222-2222-4222-8222-222222222222".into(),
        origin_timestamp: 1,
        source: CastroActionSource::User,
        action_type: CastroActionType::ClearEpisodeNew,
        event_data: None,
    };
    let (server, live) = api(SideEffectMode::Live, 1024).await;
    Mock::given(method("POST"))
        .and(path("/profile/sync/actions"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    assert!(live.post_actions(vec![action.clone()]).await.is_err());
    let requests: Vec<Request> = server.received_requests().await.unwrap();
    assert_eq!(
        String::from_utf8_lossy(&requests[0].body),
        r#"{"actions":[{"id":1,"episode_id":"11111111-1111-4111-8111-111111111111","origin_event_id":"22222222-2222-4222-8222-222222222222","origin_timestamp":1,"source":"user","action_type":"clear_episode_new"}]}"#
    );

    let (server, recorded) = api(SideEffectMode::Record, 1024).await;
    recorded.post_actions(vec![action]).await.unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(recorded.recorded_writes().len(), 1);
    assert_eq!(recorded.recorded_writes()[0].path, "/profile/sync/actions");
}

#[test]
fn encodes_search_terms_like_castro() {
    assert_eq!(
        encode_castro_query_value("Tom's (Big)*!"),
        "Tom%27s%20%28Big%29%2A%21"
    );
}
