//! Local fakes for Executor and Omni. Every server binds 127.0.0.1:0; nothing
//! leaves the machine.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::future::IntoFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use omni_events_adapter::AdapterOptions;
use omni_events_adapter::legacy::{
    ElicitAnswer, ElicitHandler, ElicitationSupport, LegacyConnector, LegacyError, LegacySession,
};
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub const TOKEN: &str = "test-oauth-token";
pub const USER_ID: &str = "owner-user";
pub const CLIENT_ID: &str = "client-id";
pub const VERSION: &str = "2026-07-28";

/// A spawned local server, aborted on drop.
pub struct Spawned {
    pub url: String,
    handle: JoinHandle<()>,
}

impl Drop for Spawned {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub async fn spawn(router: Router) -> Spawned {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = axum::serve(listener, router).into_future();
    let handle = tokio::spawn(async move {
        let _ = server.await;
    });
    Spawned { url, handle }
}

pub fn options(executor: &str, omni: &str, user: &str) -> AdapterOptions {
    AdapterOptions::new(
        executor,
        omni,
        "omni-test-token",
        user,
        "https://mcp.syas.ca",
    )
    .unwrap()
}

fn expires_in_a_minute() -> String {
    (jiff::Timestamp::now() + jiff::SignedDuration::from_secs(60)).to_string()
}

/// Better Auth's session endpoint answer for `Bearer {token}`.
pub fn session_json(headers: &HeaderMap, token: &str, user: &str, client: &str) -> Response {
    let authorized = headers.get("authorization").and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {token}"));
    let body = if authorized {
        json!({
            "userId": user,
            "clientId": client,
            "accessTokenExpiresAt": expires_in_a_minute(),
            "accessToken": "must-not-leak",
            "refreshToken": "must-not-leak",
        })
    } else {
        Value::Null
    };
    ([("content-type", "application/json")], body.to_string()).into_response()
}

/// Executor with a session endpoint and a legacy `/mcp` that echoes its route.
pub fn fake_executor() -> Router {
    Router::new().fallback(|request: Request| async move {
        let path = request.uri().path().to_owned();
        if path == "/api/auth/mcp/get-session" {
            return session_json(request.headers(), TOKEN, USER_ID, CLIENT_ID);
        }
        if path.starts_with("/mcp") {
            let route = request
                .uri()
                .path_and_query()
                .map_or(path.clone(), |p| p.as_str().to_owned());
            let body = json!({
                "legacy": true,
                "path": route,
                "method": request.method().as_str(),
                "userAgent": request.headers().get("user-agent").and_then(|v| v.to_str().ok()),
            });
            return ([("content-type", "application/json")], body.to_string()).into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    })
}

#[derive(Debug, Clone)]
pub struct Received {
    pub headers: HeaderMap,
    pub path: String,
    pub body: Value,
}

pub type Inbox = Arc<Mutex<Option<Received>>>;

/// Omni's `/mcp`: records the forwarded request and answers `events: []`.
pub fn fake_omni(inbox: Inbox) -> Router {
    Router::new().fallback(handle_omni).with_state(inbox)
}

async fn handle_omni(State(inbox): State<Inbox>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes: Bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let received = Received {
        headers: parts.headers.clone(),
        path: parts.uri.to_string(),
        body: body.clone(),
    };
    *inbox.lock().unwrap() = Some(received);
    let method = parts
        .headers
        .get("mcp-method")
        .and_then(|v| v.to_str().ok());
    if method != body["method"].as_str() {
        let error = json!({"error": {"message": "Mcp-Method header mismatch"}});
        return (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            error.to_string(),
        )
            .into_response();
    }
    let reply = json!({"jsonrpc": "2.0", "id": body["id"], "result": {"events": []}});
    ([("content-type", "application/json")], reply.to_string()).into_response()
}

pub fn action_text(answer: &ElicitAnswer) -> &'static str {
    match answer {
        ElicitAnswer::Accept(_) => "accept",
        ElicitAnswer::Decline => "decline",
        ElicitAnswer::Cancel => "cancel",
    }
}

pub fn text_result(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}]})
}

pub fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

/// What a fake tool call does: its 1-based call number and the elicitation
/// handler the continuation installed (if any).
pub type ToolBehavior = Arc<
    dyn Fn(usize, Option<ElicitHandler>) -> BoxFuture<'static, Result<Value, LegacyError>>
        + Send
        + Sync,
>;

/// A scripted legacy Executor client, counting tool calls and closes.
#[derive(Clone)]
pub struct FakeConnector {
    pub calls: Arc<AtomicUsize>,
    pub closed: Arc<AtomicUsize>,
    behavior: ToolBehavior,
}

impl FakeConnector {
    pub fn new(behavior: ToolBehavior) -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(AtomicUsize::new(0)),
            behavior,
        })
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn closed(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }
}

struct FakeSession {
    connector: FakeConnector,
    elicit: Option<ElicitHandler>,
}

impl LegacySession for FakeSession {
    fn request(
        &self,
        method: &str,
        _params: Value,
        _timeout: Duration,
    ) -> BoxFuture<'_, Result<Value, LegacyError>> {
        assert_eq!(method, "tools/call");
        let call = self.connector.calls.fetch_add(1, Ordering::SeqCst) + 1;
        (self.connector.behavior)(call, self.elicit.clone())
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        self.connector.closed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

impl LegacyConnector for FakeConnector {
    fn connect(
        &self,
        _authorization: String,
        _mode: Option<String>,
        _support: ElicitationSupport,
        elicit: Option<ElicitHandler>,
    ) -> BoxFuture<'static, Result<Arc<dyn LegacySession>, LegacyError>> {
        let session = FakeSession {
            connector: self.clone(),
            elicit,
        };
        Box::pin(async move { Ok(Arc::new(session) as Arc<dyn LegacySession>) })
    }
}

/// POSTs a modern request with matching header and metadata versions.
pub async fn modern(
    adapter: &str,
    method: &str,
    params: Value,
    auth: &str,
    path: &str,
) -> reqwest::Response {
    let mut params = params.as_object().cloned().unwrap_or_default();
    let mut meta = params
        .remove("_meta")
        .and_then(|m| m.as_object().cloned())
        .unwrap_or_default();
    meta.insert(
        "io.modelcontextprotocol/protocolVersion".into(),
        VERSION.into(),
    );
    params.insert("_meta".into(), Value::Object(meta));
    reqwest::Client::new()
        .post(format!("{adapter}{path}"))
        .header("authorization", format!("Bearer {auth}"))
        .header("content-type", "application/json")
        .header("mcp-protocol-version", VERSION)
        .body(json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}).to_string())
        .send()
        .await
        .unwrap()
}
