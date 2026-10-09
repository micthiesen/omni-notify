//! The Observer client against a wiremock Overseerr.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_arr::SideEffects;
use omni_arr::observer::{IssueFilter, ListIssuesOptions, ObserverClient, ObserverClientConfig};
use omni_http::SideEffectMode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn issue(id: i64) -> Value {
    json!({
        "id": id,
        "issueType": 1,
        "status": 1,
        "createdAt": "2026-01-01T00:00:00.000Z",
        "updatedAt": "2026-01-01T00:00:00.000Z",
        "media": { "id": 44, "tmdbId": 123, "tvdbId": null, "status": 5 },
        "comments": [{ "id": 9, "message": "broken", "user": null }],
    })
}

fn client(url: String, api_key: &str, mode: SideEffectMode) -> (ObserverClient, SideEffects) {
    let side_effects = SideEffects::new(mode);
    let client = ObserverClient::new(ObserverClientConfig {
        url,
        api_key: api_key.into(),
        http: omni_testkit::no_network(),
        side_effects: side_effects.clone(),
    });
    (client, side_effects)
}

#[tokio::test]
async fn lists_bounded_pages_and_decodes_issue_media_and_comments() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/issue"))
        .and(query_param("skip", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "pageInfo": { "page": 1, "pages": 2, "results": 2 },
            "results": [issue(1), issue(2)],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/issue"))
        .and(query_param("skip", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "pageInfo": { "page": 2, "pages": 2, "results": 2 },
            "results": [issue(3)],
        })))
        .mount(&server)
        .await;
    let (client, _) = client(format!("{}/", server.uri()), "secret", SideEffectMode::Live);

    let issues = client
        .list_issues(ListIssuesOptions {
            filter: IssueFilter::All,
            max_records: 3,
            ..ListIssuesOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(
        issues.iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]
            .url
            .as_str()
            .contains("/api/v1/issue?take=3&skip=0&filter=all")
    );
    assert_eq!(requests[0].headers.get("X-Api-Key").unwrap(), "secret");
    assert_eq!(
        issues[0].media.as_option().unwrap().id.as_option(),
        Some(&44)
    );
    assert_eq!(issues[0].comment_list()[0].message, "broken");
}

#[tokio::test]
async fn posts_comments_and_resolves_issues_with_the_documented_endpoints() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue(7)))
        .mount(&server)
        .await;
    let (client, _) = client(server.uri(), "secret", SideEffectMode::Live);

    client.add_comment(7, "Repair queued").await.unwrap();
    client.resolve_issue(7).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), "/api/v1/issue/7/comment");
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].body, br#"{"message":"Repair queued"}"#);
    assert_eq!(requests[1].url.path(), "/api/v1/issue/7/resolved");
    assert_eq!(requests[1].method.as_str(), "POST");
}

#[tokio::test]
async fn returns_a_typed_error_without_exposing_the_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "nope": true })))
        .mount(&server)
        .await;
    let (client, _) = client(server.uri(), "super-secret", SideEffectMode::Live);

    let error = client.get_issue(4).await.unwrap_err();

    assert_eq!(error.operation, "Observer GET /issue/4");
    assert!(!error.to_string().contains("super-secret"));
    assert!(client.get_issue(4).await.is_err());
}

#[tokio::test]
async fn record_mode_captures_comment_and_resolution_without_sending() {
    let server = MockServer::start().await;
    let (client, side_effects) = client(server.uri(), "secret", SideEffectMode::Record);

    assert!(client.add_comment(7, "Repair queued").await.is_err());
    assert!(client.resolve_issue(7).await.is_err());

    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(side_effects.recorded().len(), 2);
}
