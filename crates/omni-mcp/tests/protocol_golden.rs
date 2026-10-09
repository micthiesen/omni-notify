//! Replays every captured raw HTTP exchange (`tests/golden/protocol.json`, a
//! committed offline capture) against the endpoint and compares
//! status, content type, the security headers and every JSON-RPC message.
//!
//! The services match the capture: the same MCP token (so subscription ids,
//! owner digests and key ids must be bit-identical), an Executor authorizer
//! accepting one delegated owner, a webhook that verifies and accepts, and a
//! task registry that lists nothing and queues `golden-run`.
//!
//! Normalized before comparing: ISO timestamps (`refreshBefore` depends on
//! the wall clock) and the detail after "Invalid arguments for tool <name>:"
//! (the captured issue wording versus the JSON Schema validator's).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use common::*;
use futures::future::BoxFuture;
use omni_mcp::activity::ActivityRecorder;
use omni_mcp::events::executor_auth::{EventAuthorizer, ExecutorAuthError};
use omni_mcp::events::service::McpEventService;
use omni_mcp::events::webhook::{WebhookDestination, WebhookError, WebhookEvent, WebhookPort};
use omni_mcp::rpc::McpProtocol;
use omni_mcp::tools::system::{ConfiguredFeatures, SystemDeps, system_tools};
use omni_mcp_kit::McpTool;
use omni_runtime::ports::Ports;
use serde_json::Value;
use tokio_util::task::TaskTracker;

const GOLDEN_TOKEN: &str = "golden-capture-token-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const OWNER_HEX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct AcceptAll;

impl WebhookPort for AcceptAll {
    fn verify<'a>(&'a self, _: &'a WebhookDestination) -> BoxFuture<'a, Result<(), WebhookError>> {
        Box::pin(async { Ok(()) })
    }

    fn deliver<'a>(
        &'a self,
        _: &'a WebhookDestination,
        _: &'a WebhookEvent,
    ) -> BoxFuture<'a, Result<u16, WebhookError>> {
        Box::pin(async { Ok(204) })
    }
}

struct OneOwner(Arc<omni_core::clock::TestClock>);

impl EventAuthorizer for OneOwner {
    fn authorize<'a>(
        &'a self,
        owner: &'a str,
        authorization: &'a str,
    ) -> BoxFuture<'a, Result<Option<i64>, ExecutorAuthError>> {
        use omni_core::clock::Clock as _;
        let valid =
            owner == format!("executor:{OWNER_HEX}") && authorization == "Bearer delegated-fixture";
        let now = self.0.now_ms();
        Box::pin(async move { Ok(valid.then_some(now + 3_600_000)) })
    }
}

fn is_iso(text: &str) -> bool {
    text.len() == 24
        && text.ends_with('Z')
        && text.as_bytes()[10] == b'T'
        && text.as_bytes()[4] == b'-'
}

fn normalize(value: &Value) -> Value {
    match value {
        Value::String(text) if is_iso(text) => Value::String("<iso>".to_owned()),
        Value::String(text)
            if text.starts_with("Input validation error: Invalid arguments for tool ") =>
        {
            let cut = text.find(": ").map_or(text.len(), |i| {
                text[i + 2..].find(": ").map_or(text.len(), |j| i + 2 + j)
            });
            Value::String(format!("{}: <issues>", &text[..cut]))
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), normalize(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn essence(content_type: Option<&str>) -> Option<String> {
    content_type.map(|v| v.split(';').next().unwrap_or_default().trim().to_owned())
}

#[tokio::test(start_paused = true)]
async fn endpoint_reproduces_every_captured_exchange() {
    let golden: Value = serde_json::from_str(include_str!("golden/protocol.json")).unwrap();
    let clock = clock();
    let store = test_store(&clock).await;
    let events = McpEventService::new(
        GOLDEN_TOKEN,
        store.store.clone(),
        clock.clone(),
        Arc::new(AcceptAll),
        Some(Arc::new(OneOwner(clock.clone()))),
        Ports::default(),
    );
    let tasks = FakeTasks {
        run_id: Some("golden-run"),
        ..FakeTasks::default()
    };
    let own: Vec<McpTool> = system_tools(&SystemDeps {
        tasks: Arc::new(tasks),
        ports: Ports::default(),
        features: ConfiguredFeatures::default(),
        clock: clock.clone(),
    })
    .unwrap();
    let mut tools = other_tools(&own);
    tools.extend(own);
    let recorder = ActivityRecorder::new(store.store.clone(), clock.clone(), TaskTracker::new());
    let protocol = McpProtocol::new(tools, recorder, Some(events)).unwrap();
    let router = omni_mcp::endpoint::router(Some(GOLDEN_TOKEN), Some(protocol));

    let mut compared = 0;
    for exchange in golden["exchanges"].as_array().unwrap() {
        let label = exchange["label"].as_str().unwrap();
        let req = &exchange["request"];
        let mut builder = Request::builder()
            .method(req["method"].as_str().unwrap())
            .uri("/mcp");
        let token = if label == "raw unauthorized" {
            "wrong"
        } else {
            GOLDEN_TOKEN
        };
        builder = builder.header("authorization", format!("Bearer {token}"));
        for (name, value) in req["headers"].as_object().unwrap() {
            builder = builder.header(name.as_str(), value.as_str().unwrap());
        }
        let body = match &req["body"] {
            Value::Null => Body::empty(),
            Value::String(raw) => Body::from(raw.clone()),
            other => Body::from(other.to_string()),
        };
        let actual = send(&router, builder.body(body).unwrap()).await;
        let expected = &exchange["response"];
        assert_eq!(
            actual.status.as_u16(),
            expected["status"].as_u64().unwrap() as u16,
            "{label}: status ({})",
            actual.text
        );
        assert_eq!(
            essence(actual.header("content-type")),
            essence(expected["contentType"].as_str()),
            "{label}: content type"
        );
        for (name, value) in expected["headers"].as_object().unwrap() {
            assert_eq!(
                actual.header(name),
                value.as_str(),
                "{label}: header {name}"
            );
        }
        let messages = normalize(&Value::Array(actual.messages()));
        let wanted = normalize(&expected["messages"]);
        assert_eq!(
            omni_core::js::json_stringify(&messages),
            omni_core::js::json_stringify(&wanted),
            "{label}: messages"
        );
        compared += 1;
    }
    assert_eq!(compared, golden["exchanges"].as_array().unwrap().len());
}
