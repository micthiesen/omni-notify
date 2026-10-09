//! Port of `src/mcp/events/executorAuth.spec.ts` (all cases kept) against a
//! wiremock session endpoint reached through the offline client's rewrite.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_core::clock::TestClock;
use omni_mcp::events::executor_auth::{
    EventAuthorizer, ExecutorEventAuthorizer, executor_owner_id,
};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NOW: i64 = 1_790_000_000_000;
const SESSION_URL: &str = "http://executor:4788/api/auth/mcp/get-session";

async fn authorizer(server: &MockServer) -> ExecutorEventAuthorizer {
    ExecutorEventAuthorizer::new(
        SESSION_URL,
        omni_testkit::mock_http(server, &["http://executor:4788"]),
        TestClock::new(NOW),
    )
    .unwrap()
}

#[tokio::test]
async fn distinguishes_temporary_http_failures_from_revoked_access() {
    let owner = executor_owner_id("user", "client");
    for status in [429, 500, 503] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status).set_body_string("unavailable"))
            .mount(&server)
            .await;
        let result = authorizer(&server)
            .await
            .authorize(&owner, "Bearer opaque")
            .await;
        assert!(result.is_err(), "{status}");
    }
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    assert_eq!(
        authorizer(&server)
            .await
            .authorize(&owner, "Bearer opaque")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn requires_the_exact_user_client_pair_and_a_live_access_token() {
    let expires_at = omni_core::js::to_iso_string(NOW + 60_000);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/auth/mcp/get-session"))
        .and(header("authorization", "Bearer opaque"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": "user-1",
            "clientId": "client-2",
            "accessTokenExpiresAt": expires_at,
            "accessToken": "private-upstream-value",
        })))
        .expect(2)
        .mount(&server)
        .await;
    let authorize = authorizer(&server).await;
    let first = authorize
        .authorize(&executor_owner_id("user-1", "client-2"), "Bearer opaque")
        .await;
    let sent = server.received_requests().await.unwrap();
    assert_eq!(
        sent[0].headers.get("user-agent").unwrap(),
        "OpenAI File Downloader, XaiImageApiFetch/1.0"
    );
    assert_eq!(first.unwrap(), Some(NOW + 60_000));
    assert_eq!(
        authorize
            .authorize(&executor_owner_id("user-1", "other"), "Bearer opaque")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn rejects_expired_malformed_and_missing_sessions() {
    let owner = executor_owner_id("user", "client");
    for body in [
        json!(null),
        json!({"userId": "user", "clientId": "client", "accessTokenExpiresAt": "2020-01-01T00:00:00Z"}),
        json!({"userId": "user", "clientId": "client", "accessTokenExpiresAt": "not-a-date"}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
            .mount(&server)
            .await;
        assert_eq!(
            authorizer(&server)
                .await
                .authorize(&owner, "Bearer opaque")
                .await
                .unwrap(),
            None,
            "{body}"
        );
    }
}

#[test]
fn owner_ids_hash_the_json_pair() {
    assert_eq!(
        executor_owner_id("user", "client"),
        format!(
            "executor:{}",
            omni_core::digest::sha256_hex(r#"["user","client"]"#)
        )
    );
    assert!(
        ExecutorEventAuthorizer::new(
            "ftp://executor/x",
            omni_testkit::no_network(),
            TestClock::new(NOW)
        )
        .is_err()
    );
    assert!(
        ExecutorEventAuthorizer::new(
            "http://user:pass@executor/x",
            omni_testkit::no_network(),
            TestClock::new(NOW)
        )
        .is_err()
    );
}
