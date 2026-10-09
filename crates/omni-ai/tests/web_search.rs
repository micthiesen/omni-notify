//! Port of `src/ai/tools/webSearch.spec.ts`. The TS spec injects a streaming request
//! function; here a local wiremock server stands in for Tavily (the plain client is
//! used because the public client refuses loopback addresses).
#![allow(clippy::unwrap_used)]

use omni_ai::AiTool;
use omni_ai::costs::{CostEventData, CostRecorder};
use omni_ai::tools::{SearchOptions, WebSearch, WebSearchError, WebSearchResult, WebSearchResults};
use omni_core::clock::SharedClock;
use omni_http::HttpError;
use omni_store::EntityOps;
use omni_tasks::EventBus;
use omni_tasks::log_capture::{RunLogLayer, RunLogs, run_span};
use omni_testkit::{TEST_EPOCH_MS, TestStore, mock_http, mock_server, test_clock};
use serde_json::json;
use tracing::Instrument;
use tracing_subscriber::layer::SubscriberExt;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

struct Setup {
    store: TestStore,
    clock: SharedClock,
    server: wiremock::MockServer,
}

async fn setup() -> Setup {
    let clock: SharedClock = test_clock(TEST_EPOCH_MS);
    Setup {
        store: TestStore::new(clock.clone()).await,
        clock,
        server: mock_server().await,
    }
}

impl Setup {
    fn search(&self) -> WebSearch {
        WebSearch::with_http(
            mock_http(&self.server, &["https://api.tavily.com"]),
            "tvly-test".to_owned(),
            CostRecorder::new(self.store.store.clone(), self.clock.clone()),
        )
    }

    async fn reply(&self, body: String) {
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&self.server)
            .await;
    }
}

#[tokio::test]
async fn streams_and_decodes_the_bounded_tavily_response() {
    let s = setup().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("authorization", "Bearer tvly-test"))
        .and(body_json(json!({"query": "test", "max_results": 5})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"title": "Result", "url": "https://example.com", "content": "excerpt"}],
            "response_time": 0.25
        })))
        .mount(&s.server)
        .await;
    let results = s
        .search()
        .search(SearchOptions {
            query: "test".to_owned(),
            ..SearchOptions::default()
        })
        .await;
    let results = results.unwrap();
    // wiremock's header matcher splits on commas, so the UA is checked here.
    let received = s.server.received_requests().await.unwrap();
    assert_eq!(
        received[0].headers.get("user-agent").unwrap(),
        omni_http::USER_AGENT
    );
    assert_eq!(
        results,
        WebSearchResults {
            results: vec![WebSearchResult {
                title: "Result".to_owned(),
                url: "https://example.com".to_owned(),
                content: "excerpt".to_owned(),
            }],
            response_time: 0.25,
        }
    );
}

#[tokio::test]
async fn rejects_a_response_that_exceeds_the_byte_limit_while_streaming() {
    let s = setup().await;
    s.reply("1234567890".to_owned()).await;
    let result = s
        .search()
        .with_max_response_bytes(8)
        .search(SearchOptions {
            query: "test".to_owned(),
            ..SearchOptions::default()
        })
        .await;
    assert!(matches!(
        result,
        Err(WebSearchError::Http(HttpError::TooLarge { limit: 8 }))
    ));
}

#[tokio::test]
async fn rejects_a_structurally_invalid_provider_response() {
    let s = setup().await;
    s.reply(json!({"results": [{"title": 42}], "response_time": 1}).to_string())
        .await;
    let error = s
        .search()
        .search(SearchOptions {
            query: "test".to_owned(),
            ..SearchOptions::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(error, WebSearchError::Decode(_)));
    assert!(error.to_string().starts_with("Web search failed"));
}

#[tokio::test]
async fn retains_task_attribution_when_the_tool_is_invoked_later() {
    let s = setup().await;
    s.reply(json!({"results": [], "response_time": 0.1}).to_string())
        .await;
    let logs = RunLogs::new(EventBus::new(16), s.clock.clone());
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(RunLogLayer::new(logs)),
    );
    let tool = s.search();
    let run_id = "web-search-tool-context";
    tool.call(json!({"query": "context test"}))
        .instrument(run_span(run_id, "Recommendations"))
        .await
        .unwrap();
    let events = s
        .store
        .store
        .read(|docs| docs.get_all::<CostEventData>())
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].run_id.as_deref(), Some(run_id));
    assert_eq!(events[0].feature, "media-recommendations");
    assert_eq!(events[0].service, "tavily");
    assert_eq!(events[0].cost_cents, Some(0.8));
}
