//! Port of `packages/executor-events-adapter/test/native.node-test.mjs`.
//!
//! The TS case paired the TS SDK's legacy server with the MCP 2 client. Here the
//! legacy Executor is rmcp's Streamable HTTP server (an independent MCP
//! implementation, in legacy session mode) and the modern client's two rounds
//! (`tools/call` returning `input_required`, then the reply carrying
//! `requestState` and `inputResponses`) are sent by hand, since no Rust MCP 2
//! client exists.
//!
//! `legacy_elicitation_on_the_standalone_stream_completes` is Rust-only: the TS
//! fake server issued its elicitation outside the tool request, which the TS
//! SDK delivers on the standalone GET stream; rmcp associates it with the
//! request stream, so a hand-written server covers the GET path.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use omni_events_adapter::AdapterBuilder;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequestParams,
    ElicitationAction, ElicitationSchema, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};
use support::*;
use tokio::sync::{Mutex, mpsc};

#[derive(Clone)]
struct FakeExecutor {
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for FakeExecutor {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        assert_eq!(request.name, "confirm");
        self.calls.fetch_add(1, Ordering::SeqCst);
        let schema = ElicitationSchema::builder()
            .required_bool("yes")
            .build()
            .map_err(|e| ErrorData::internal_error(e, None))?;
        let reply = context
            .peer
            .create_elicitation(ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: "Confirm?".into(),
                requested_schema: schema,
            })
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let accepted = reply.action == ElicitationAction::Accept
            && reply.content.as_ref().and_then(|c| c.get("yes")) == Some(&json!(true));
        let text = if accepted { "accepted" } else { "declined" };
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
    }
}

fn executor_router(calls: Arc<AtomicUsize>) -> Router {
    let mcp = StreamableHttpService::new(
        move || {
            Ok(FakeExecutor {
                calls: calls.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route(
            "/api/auth/mcp/get-session",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                session_json(&headers, "test", "u", "c")
            }),
        )
        .nest_service("/mcp", mcp)
}

async fn modern_tool_call(adapter: &str, params: Value) -> Value {
    let params = json!({
        "name": params["name"],
        "arguments": params["arguments"],
        "requestState": params.get("requestState"),
        "inputResponses": params.get("inputResponses"),
        "_meta": {
            "io.modelcontextprotocol/clientCapabilities": {"elicitation": {"form": {}}},
            "io.modelcontextprotocol/clientInfo": {"name": "native-test", "version": "1.0.0"},
        },
    });
    let mut params = params.as_object().cloned().unwrap();
    params.retain(|_, v| !v.is_null());
    let response = modern(
        adapter,
        "tools/call",
        Value::Object(params),
        "test",
        "/mcp?elicitation_mode=native",
    )
    .await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

/// Runs the modern two-round elicitation against `adapter` and returns the text.
async fn confirm_through(adapter: &str) -> String {
    let first = modern_tool_call(adapter, json!({"name": "confirm", "arguments": {}})).await;
    let result = &first["result"];
    assert_eq!(result["resultType"], "input_required", "{first}");
    let elicitation = &result["inputRequests"]["elicitation"];
    assert_eq!(elicitation["method"], "elicitation/create");
    assert_eq!(elicitation["params"]["message"], "Confirm?");
    let reply = json!({
        "name": "confirm",
        "arguments": {},
        "requestState": result["requestState"],
        "inputResponses": {"elicitation": {"action": "accept", "content": {"yes": true}}},
    });
    let done = modern_tool_call(adapter, reply).await;
    assert_eq!(done["result"]["resultType"], "complete", "{done}");
    done["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn mcp_2_native_elicitation_completes_through_a_real_legacy_mcp_session() {
    let calls = Arc::new(AtomicUsize::new(0));
    let upstream = spawn(executor_router(calls.clone())).await;
    let built = AdapterBuilder::new(options(&upstream.url, &upstream.url, "u"))
        .build()
        .unwrap();
    let continuations = built.continuations().clone();
    let adapter = spawn(built.router()).await;
    assert_eq!(confirm_through(&adapter.url).await, "accepted");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(continuations.active(), 0);
}

#[tokio::test]
async fn legacy_methods_bridge_through_a_real_legacy_mcp_session() {
    let calls = Arc::new(AtomicUsize::new(0));
    let upstream = spawn(executor_router(calls)).await;
    let built = AdapterBuilder::new(options(&upstream.url, &upstream.url, "u"))
        .build()
        .unwrap();
    let adapter = spawn(built.router()).await;
    let response = modern(&adapter.url, "tools/list", json!({}), "test", "/mcp").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(body["result"]["tools"], json!([]));
    assert_eq!(body["result"]["ttlMs"], 0);
    assert_eq!(body["result"]["cacheScope"], "private");
    assert!(body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"].is_object());
}

/// The open `tools/call` response stream and its request id.
type ToolStream = (Value, mpsc::UnboundedSender<String>);

/// A minimal legacy server that sends its elicitation on the GET stream and
/// answers the tool call on the POST stream once the client has replied.
#[derive(Clone, Default)]
struct StandaloneServer {
    get_stream: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
    tool_stream: Arc<Mutex<Option<ToolStream>>>,
    calls: Arc<AtomicUsize>,
    elicitation_mode: Arc<Mutex<Option<String>>>,
}

fn sse(rx: mpsc::UnboundedReceiver<String>) -> Response {
    let stream =
        tokio_stream_from(rx).map(|data| Ok::<_, std::io::Error>(format!("data: {data}\n\n")));
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

fn tokio_stream_from(
    mut rx: mpsc::UnboundedReceiver<String>,
) -> impl futures::Stream<Item = String> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

async fn standalone(State(server): State<StandaloneServer>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    if parts.uri.path() == "/api/auth/mcp/get-session" {
        return session_json(&parts.headers, "test", "u", "c");
    }
    if parts.method == Method::GET {
        assert_eq!(parts.headers["mcp-session-id"], "s-1");
        assert_eq!(parts.headers["mcp-protocol-version"], "2025-06-18");
        let (tx, rx) = mpsc::unbounded_channel();
        *server.get_stream.lock().await = Some(tx);
        return sse(rx);
    }
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let message: Value = serde_json::from_slice(&bytes).unwrap();
    match message["method"].as_str() {
        Some("initialize") => {
            let mode = parts.uri.query().map(str::to_owned);
            *server.elicitation_mode.lock().await = mode;
            assert_eq!(
                message["params"]["capabilities"],
                json!({"elicitation": {"form": {}}})
            );
            let reply = json!({"jsonrpc": "2.0", "id": message["id"], "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "standalone", "version": "1"},
            }});
            (
                [
                    ("content-type", "application/json"),
                    ("mcp-session-id", "s-1"),
                ],
                reply.to_string(),
            )
                .into_response()
        }
        Some("notifications/initialized") => StatusCode::ACCEPTED.into_response(),
        Some("tools/call") => {
            server.calls.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = mpsc::unbounded_channel();
            *server.tool_stream.lock().await = Some((message["id"].clone(), tx));
            let elicit = json!({"jsonrpc": "2.0", "id": "e-1", "method": "elicitation/create",
                "params": {"message": "Confirm?", "requestedSchema": {"type": "object",
                    "properties": {"yes": {"type": "boolean"}}}}});
            if let Some(get) = server.get_stream.lock().await.as_ref() {
                get.send(elicit.to_string()).unwrap();
            }
            sse(rx)
        }
        None if message["id"] == "e-1" => {
            let accepted = message["result"]["action"] == "accept"
                && message["result"]["content"]["yes"] == true;
            let text = if accepted { "accepted" } else { "declined" };
            if let Some((id, tx)) = server.tool_stream.lock().await.take() {
                let result = json!({"jsonrpc": "2.0", "id": id, "result":
                    {"content": [{"type": "text", "text": text}]}});
                tx.send(result.to_string()).unwrap();
            }
            StatusCode::ACCEPTED.into_response()
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

#[tokio::test]
async fn legacy_elicitation_on_the_standalone_stream_completes() {
    let server = StandaloneServer::default();
    let upstream = spawn(
        Router::new()
            .fallback(standalone)
            .with_state(server.clone()),
    )
    .await;
    let built = AdapterBuilder::new(options(&upstream.url, &upstream.url, "u"))
        .continuation_ttl(Duration::from_secs(10))
        .build()
        .unwrap();
    let adapter = spawn(built.router()).await;
    assert_eq!(confirm_through(&adapter.url).await, "accepted");
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.elicitation_mode.lock().await.as_deref(),
        Some("elicitation_mode=native")
    );
}
