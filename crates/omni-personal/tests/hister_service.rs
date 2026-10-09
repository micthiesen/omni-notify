//! The Hister service against a wiremock Hister. Timeouts run on tokio's
//! paused clock; dropping a call cancels its request promptly, since a reqwest
//! future aborts its request when dropped.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use omni_http::SideEffectMode;
use omni_personal::hister::{BrowseInput, HisterService, PageInput, SearchInput};
use omni_testkit::{mock_http, mock_server};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "secret-token";

fn service(server: &MockServer) -> HisterService {
    HisterService::new(
        "https://hister.test",
        TOKEN,
        mock_http(server, &["https://hister.test"]),
        SideEffectMode::Live,
    )
    .unwrap()
}

fn search(query: &str) -> SearchInput {
    SearchInput {
        query: query.into(),
        ..SearchInput::default()
    }
}

#[tokio::test]
async fn searches_with_bounded_json_preserves_cursors_and_separates_history_selections() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(header("x-access-token", TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 1,
            "page_key": "next",
            "history": [{
                "id": "history-1", "url": "https://example.test/selected",
                "title": "Previously selected", "updated": 1_700_000_000
            }],
            "documents": [{
                "id": "document-1", "url": "https://example.test/a", "title": "A", "snippet": "snippet"
            }]
        })))
        .mount(&server)
        .await;
    let result = service(&server)
        .search(&SearchInput {
            query: "term".into(),
            limit: Some(1),
            cursor: Some("cursor-in".into()),
            date_from: Some("2026-01-01".into()),
            date_to: Some("2026-01-02".into()),
            semantic: Some(true),
        })
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let url = &requests[0].url;
    let query: Value = serde_json::from_str(
        &url.query_pairs()
            .find(|(k, _)| k == "query")
            .map(|(_, v)| v.into_owned())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        query,
        json!({
            "text": "term", "limit": 1, "include_html": false, "include_text": true,
            "semantic_enabled": true, "page_key": "cursor-in",
            "date_from": 1_767_225_600, "date_to": 1_767_398_399
        })
    );
    assert_eq!(result.total, 1.0);
    assert_eq!(result.next_cursor.as_deref(), Some("next"));
    let first = &result.results[0];
    assert_eq!(first.document_id.as_deref(), Some("document-1"));
    assert_eq!(first.title, "A");
    assert_eq!(first.url, "https://example.test/a");
    assert_eq!(first.snippet, "snippet");
    assert!(!first.title_truncated && !first.snippet_truncated);
    assert_eq!(result.prior_selections.len(), 1);
    let prior = &result.prior_selections[0];
    assert_eq!(prior.document_id.as_deref(), Some("history-1"));
    assert_eq!(prior.title, "Previously selected");
    assert_eq!(prior.snippet, "");
    assert_eq!(prior.updated_at, Some(1_700_000_000.0));
    let note = result.prior_selections_note.to_lowercase();
    assert!(note.contains("not constrained") && note.contains("matching"));
}

#[tokio::test]
async fn round_trips_the_indexed_history_cursor() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/api/history"))
        .and(wiremock::matchers::query_param("last", "cursor-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "documents": [{"url": "https://example.test/old", "title": "Old", "updated": 1_700_000_001}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page_key": "cursor-1",
            "documents": [{"url": "https://example.test/new", "title": "New", "updated": 1_700_000_002}]
        })))
        .mount(&server)
        .await;
    let hister = service(&server);
    let first = hister
        .browse(&BrowseInput {
            filter: Some("term".into()),
            ..BrowseInput::default()
        })
        .await
        .unwrap();
    let second = hister
        .browse(&BrowseInput {
            filter: Some("term".into()),
            cursor: first.next_cursor.clone(),
            ..BrowseInput::default()
        })
        .await
        .unwrap();
    assert_eq!(first.next_cursor.as_deref(), Some("cursor-1"));
    assert_eq!(second.items[0].url, "https://example.test/old");
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].url.as_str().contains("filter=term"));
    assert!(requests[1].url.as_str().contains("filter=term"));
    assert!(requests[1].url.as_str().contains("last=cursor-1"));
}

#[tokio::test]
async fn slices_page_text_and_never_requests_raw_html() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/api/document"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "url": "https://example.test/a", "title": "A", "text": "abcdefghij"
        })))
        .mount(&server)
        .await;
    let result = service(&server)
        .get_page(&PageInput {
            url: "https://example.test/a".into(),
            offset: Some(2),
            max_chars: Some(3),
            ..PageInput::default()
        })
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].url.as_str().contains("/api/document?"));
    assert!(!requests[0].url.as_str().contains("html"));
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "title": "A", "titleTruncated": false, "url": "https://example.test/a",
            "text": "cde", "totalChars": 10, "offset": 2, "nextOffset": 5
        })
    );
}

#[tokio::test]
async fn truncates_snippets_and_titles_at_their_protocol_bounds() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 1,
            "documents": [{"url": "https://example.test/a", "title": "t".repeat(1001), "snippet": "s".repeat(1501)}]
        })))
        .mount(&server)
        .await;
    let result = service(&server).search(&search("term")).await.unwrap();
    assert_eq!(result.results[0].title.len(), 1000);
    assert!(result.results[0].title_truncated);
    assert_eq!(result.results[0].snippet.len(), 1500);
    assert!(result.results[0].snippet_truncated);
}

#[tokio::test]
async fn rejects_urls_longer_than_the_bounded_result_field() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 1,
            "documents": [{"url": format!("https://example.test/{}", "x".repeat(8192)), "title": "A"}]
        })))
        .mount(&server)
        .await;
    assert!(service(&server).search(&search("term")).await.is_err());
}

#[tokio::test]
async fn cancels_an_oversized_response_body() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; 8 * 1024 * 1024 + 1]))
        .mount(&server)
        .await;
    let error = service(&server).search(&search("term")).await.unwrap_err();
    assert_eq!(error.reason, "request failed");
}

#[tokio::test]
async fn times_out_hanging_requests_on_the_paused_clock() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .mount(&server)
        .await;
    let hister = service(&server);
    tokio::time::pause();
    let input = search("term");
    let error = hister.search(&input).await.unwrap_err();
    assert_eq!(error.reason, "request timed out");
}

#[tokio::test]
async fn aborts_an_in_flight_request_when_interrupted() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .mount(&server)
        .await;
    let hister = std::sync::Arc::new(service(&server));
    let handle = {
        let hister = hister.clone();
        tokio::spawn(async move { hister.search(&search("term")).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.abort();
    let joined = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .unwrap();
    assert!(joined.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn does_not_leak_access_tokens_or_response_bodies_on_http_errors() {
    let server = mock_server().await;
    let secret_body = format!("private-body-{TOKEN}");
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"secretBody": secret_body})))
        .mount(&server)
        .await;
    let error = service(&server).search(&search("term")).await.unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(TOKEN));
    assert!(!shown.contains(&secret_body));
    assert_eq!(error.reason, "HTTP 500");
}

#[tokio::test]
async fn refuses_redirects_so_the_token_is_never_forwarded() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://elsewhere.test/"),
        )
        .mount(&server)
        .await;
    let error = service(&server).search(&search("term")).await.unwrap_err();
    assert_eq!(error.reason, "request failed");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn fails_with_typed_errors_for_malformed_response_schemas() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"total": "bad", "documents": []})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/document"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"url": 42})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"documents": "bad"})))
        .mount(&server)
        .await;
    let hister = service(&server);
    assert_eq!(
        hister.search(&search("term")).await.unwrap_err().reason,
        "invalid response"
    );
    let page = hister
        .get_page(&PageInput {
            url: "https://example.test/a".into(),
            ..PageInput::default()
        })
        .await;
    assert_eq!(page.unwrap_err().reason, "invalid response");
    assert_eq!(
        hister
            .browse(&BrowseInput::default())
            .await
            .unwrap_err()
            .reason,
        "invalid response"
    );
}

async fn label_server(stored_label: &str) -> MockServer {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/api/label"))
        .and(header("origin", "hister://"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/document"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "url": "https://example.test/a", "label": stored_label
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn clears_labels_with_one_write_and_does_not_retry_a_verification_mismatch() {
    let clear = label_server("").await;
    let cleared = service(&clear)
        .set_label("https://example.test/a", "")
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&cleared).unwrap(),
        json!({"url": "https://example.test/a", "label": "", "verified": true})
    );
    assert_eq!(clear.received_requests().await.unwrap().len(), 2);
    let body: Value =
        serde_json::from_slice(&clear.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body, json!({"url": "https://example.test/a", "label": ""}));

    let mismatch = label_server("other").await;
    let error = service(&mismatch)
        .set_label("https://example.test/a", "wanted")
        .await
        .unwrap_err();
    assert_eq!(error.reason, "label mismatch");
    assert_eq!(mismatch.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn records_label_writes_without_sending_them_in_record_mode() {
    let server = label_server("x").await;
    let hister = HisterService::new(
        "https://hister.test",
        TOKEN,
        mock_http(&server, &["https://hister.test"]),
        SideEffectMode::Record,
    )
    .unwrap();
    assert!(
        hister
            .set_label("https://example.test/a", "x")
            .await
            .is_err()
    );
    assert_eq!(hister.recorded_labels().len(), 1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn rejects_base_urls_with_credentials_query_or_fragment() {
    let http = omni_testkit::no_network();
    for bad in ["ftp://h", "https://u:p@h", "https://h/?q=1", "https://h/#f"] {
        assert!(HisterService::new(bad, TOKEN, http.clone(), SideEffectMode::Live).is_err());
    }
}

#[tokio::test]
async fn mcp_tools_return_outputs_that_match_the_golden_schemas() {
    use omni_mcp_kit::{ToolContext, ToolOutput};
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 1,
            "documents": [{"id": "d", "url": "https://example.test/a", "title": "A", "text": "body", "score": 1.5, "updated": 1}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(Value::Null))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/document"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "url": "https://example.test/a", "text": "abc", "label": "kept"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/label"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;
    let tools =
        omni_personal::mcp::browser_history::tools(Some(std::sync::Arc::new(service(&server))))
            .unwrap();
    let call = |name: &'static str, input: Value| {
        let tool = tools.iter().find(|t| t.meta.name == name).unwrap().clone();
        async move {
            let cx = ToolContext {
                call_id: "1".into(),
                cancel: tokio_util::sync::CancellationToken::new(),
            };
            match tool.handler.call(input, cx).await.unwrap() {
                ToolOutput::Structured(map)
                | ToolOutput::Custom {
                    structured: map, ..
                } => Value::Object(map),
            }
        }
    };
    let found = call("search_browser_history", json!({"query": " term "})).await;
    assert_eq!(found["results"][0]["snippet"], json!("body"));
    assert_eq!(found["total"], json!(1));
    let browsed = call("browse_browser_history", json!({})).await;
    assert_eq!(browsed, json!({"items": []}));
    let page = call("get_browser_page", json!({"url": "https://example.test/a"})).await;
    assert_eq!(page["title"], json!("https://example.test/a"));
    assert_eq!(page["totalChars"], json!(3));
    let labeled = call(
        "set_browser_page_label",
        json!({"url": "https://example.test/a", "label": " kept "}),
    )
    .await;
    assert_eq!(labeled["verified"], json!(true));
    let requests = server.received_requests().await.unwrap();
    let query = requests[0]
        .url
        .query_pairs()
        .find(|(k, _)| k == "query")
        .unwrap()
        .1
        .into_owned();
    assert!(query.contains("\"text\":\"term\""), "{query}");
}
