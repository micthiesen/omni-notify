//! Streamable HTTP MCP client for Executor's legacy endpoint.
//!
//! Behaves like the TypeScript MCP SDK 1.29 client that Executor's legacy
//! endpoint was built against: `initialize` with protocol
//! 2025-11-25, `notifications/initialized`, an optional standalone GET event
//! stream, requests answered by JSON or by a per-request event stream, and
//! server-to-client requests (`elicitation/create`, `ping`) answered by POST.
//! Results are passed through as raw JSON. Closing aborts every stream without
//! a session DELETE.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use url::Url;

use super::sse::SseDecoder;
use super::{
    CLOSE_TIMEOUT, CONNECT_TIMEOUT, ElicitAnswer, ElicitHandler, ElicitationSupport,
    LegacyConnector, LegacyError, LegacySession,
};
use crate::config::USER_AGENT as ADAPTER_USER_AGENT;

/// Protocol versions accepted from `initialize` (those SDK 1.29 accepts).
pub const LEGACY_PROTOCOL_VERSIONS: [&str; 5] = [
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];
const CLIENT_PROTOCOL_VERSION: &str = "2025-11-25";
const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";
const LAST_EVENT_HEADER: &str = "last-event-id";
const STREAM_READY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 64 * 1024;
const GET_RECONNECT_DELAYS: [Duration; 2] =
    [Duration::from_millis(1000), Duration::from_millis(1500)];
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;

/// Connects real legacy sessions to `EXECUTOR_BASE_URL/mcp`.
#[derive(Clone)]
pub struct HttpConnector {
    http: reqwest::Client,
    executor_base_url: Url,
}

impl HttpConnector {
    pub fn new(http: reqwest::Client, executor_base_url: Url) -> Self {
        Self {
            http,
            executor_base_url,
        }
    }
}

impl LegacyConnector for HttpConnector {
    fn connect(
        &self,
        authorization: String,
        mode: Option<String>,
        support: ElicitationSupport,
        elicit: Option<ElicitHandler>,
    ) -> BoxFuture<'static, Result<Arc<dyn LegacySession>, LegacyError>> {
        let this = self.clone();
        Box::pin(async move {
            let mut endpoint = this
                .executor_base_url
                .join("/mcp")
                .map_err(|e| LegacyError::Connect(e.to_string()))?;
            // The connection's selected elicitation mode belongs to the tool call.
            if let Some(mode) =
                mode.filter(|m| matches!(m.as_str(), "native" | "browser" | "model"))
            {
                endpoint
                    .query_pairs_mut()
                    .append_pair("elicitation_mode", &mode);
            }
            let elicit = if support.any() { elicit } else { None };
            let client =
                LegacyClient::connect(this.http, endpoint, authorization, support, elicit).await?;
            Ok(Arc::new(client) as Arc<dyn LegacySession>)
        })
    }
}

type Reply = Result<Value, LegacyError>;

struct Inner {
    http: reqwest::Client,
    endpoint: Url,
    authorization: String,
    support: ElicitationSupport,
    elicit: Option<ElicitHandler>,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    pending: Mutex<HashMap<i64, oneshot::Sender<Reply>>>,
    next_id: AtomicI64,
    cancel: CancellationToken,
    tracker: TaskTracker,
}

/// A connected legacy session. Dropping it cancels its background streams.
pub struct LegacyClient {
    inner: Arc<Inner>,
}

impl Drop for LegacyClient {
    fn drop(&mut self) {
        self.inner.cancel.cancel();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl LegacyClient {
    async fn connect(
        http: reqwest::Client,
        endpoint: Url,
        authorization: String,
        support: ElicitationSupport,
        elicit: Option<ElicitHandler>,
    ) -> Result<Self, LegacyError> {
        let client = Self {
            inner: Arc::new(Inner {
                http,
                endpoint,
                authorization,
                support,
                elicit,
                session_id: Mutex::new(None),
                protocol_version: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                next_id: AtomicI64::new(0),
                cancel: CancellationToken::new(),
                tracker: TaskTracker::new(),
            }),
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, client.handshake()).await {
            Ok(Ok(())) => Ok(client),
            Ok(Err(error)) => {
                client.shutdown().await;
                Err(LegacyError::Connect(error.to_string()))
            }
            Err(_) => {
                client.shutdown().await;
                Err(LegacyError::Connect("initialize timed out".into()))
            }
        }
    }

    async fn handshake(&self) -> Result<(), LegacyError> {
        let inner = &self.inner;
        let result = Inner::request(
            inner,
            "initialize",
            json!({
                "protocolVersion": CLIENT_PROTOCOL_VERSION,
                "capabilities": inner.support.capabilities(),
                "clientInfo": {"name": "omni-executor-events-adapter", "version": "0.1.0"},
            }),
        )
        .await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|v| LEGACY_PROTOCOL_VERSIONS.contains(v))
            .ok_or_else(|| {
                LegacyError::Connect("server's protocol version is not supported".into())
            })?;
        *lock(&inner.protocol_version) = Some(version.to_owned());
        let response = inner
            .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        drop(response);
        Inner::open_standalone_stream(inner).await;
        Ok(())
    }

    async fn shutdown(&self) {
        let inner = &self.inner;
        inner.cancel.cancel();
        lock(&inner.pending).clear();
        inner.tracker.close();
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, inner.tracker.wait()).await;
    }
}

impl LegacySession for LegacyClient {
    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<Value, LegacyError>> {
        let method = method.to_owned();
        Box::pin(async move {
            match tokio::time::timeout(timeout, Inner::request(&self.inner, &method, params)).await
            {
                Ok(reply) => reply,
                Err(_) => Err(LegacyError::Timeout),
            }
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.shutdown())
    }
}

/// Removes a request's reply slot however its future ends.
struct PendingSlot<'a> {
    inner: &'a Inner,
    id: i64,
}

impl Drop for PendingSlot<'_> {
    fn drop(&mut self) {
        lock(&self.inner.pending).remove(&self.id);
    }
}

impl Inner {
    fn headers(&self, accept: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, reqwest::header::HeaderValue::from_static(accept));
        headers.insert(
            USER_AGENT,
            reqwest::header::HeaderValue::from_static(ADAPTER_USER_AGENT),
        );
        if let Ok(value) = self.authorization.parse() {
            headers.insert(AUTHORIZATION, value);
        }
        if let Some(value) = lock(&self.session_id)
            .as_deref()
            .and_then(|v| v.parse().ok())
        {
            headers.insert(SESSION_HEADER, value);
        }
        if let Some(value) = lock(&self.protocol_version)
            .as_deref()
            .and_then(|v| v.parse().ok())
        {
            headers.insert(VERSION_HEADER, value);
        }
        headers
    }

    fn remember_session(&self, response: &reqwest::Response) {
        if let Some(id) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            *lock(&self.session_id) = Some(id.to_owned());
        }
    }

    /// POSTs one JSON-RPC message; non-2xx is an error.
    async fn post(&self, message: &Value) -> Result<reqwest::Response, LegacyError> {
        let body = serde_json::to_vec(message).map_err(|e| LegacyError::Request(e.to_string()))?;
        let send = self
            .http
            .post(self.endpoint.clone())
            .headers(self.headers("application/json, text/event-stream"))
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send();
        let response = tokio::select! {
            () = self.cancel.cancelled() => return Err(LegacyError::Closed),
            response = send => response.map_err(|e| LegacyError::Request(e.to_string()))?,
        };
        self.remember_session(&response);
        if !response.status().is_success() {
            let status = response.status();
            let text = bounded_text(response).await;
            return Err(LegacyError::Request(format!("HTTP {status}: {text}")));
        }
        Ok(response)
    }

    async fn request(self: &Arc<Self>, method: &str, params: Value) -> Reply {
        if self.cancel.is_cancelled() {
            return Err(LegacyError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        let _slot = PendingSlot { inner: self, id };
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            message["params"] = params;
        }
        let work = async {
            let response = self.post(&message).await?;
            if response.status() == StatusCode::ACCEPTED {
                return (&mut rx).await.map_err(|_| LegacyError::Closed)?;
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if content_type.contains("text/event-stream") {
                let mut stream = response.bytes_stream();
                let mut decoder = SseDecoder::new(MAX_MESSAGE_BYTES);
                loop {
                    tokio::select! {
                        reply = &mut rx => return reply.map_err(|_| LegacyError::Closed)?,
                        chunk = stream.next() => match chunk {
                            Some(Ok(bytes)) => {
                                let events = decoder
                                    .push(&bytes)
                                    .map_err(|e| LegacyError::Request(e.to_string()))?;
                                for event in events.iter().filter(|e| e.is_message()) {
                                    self.dispatch_text(&event.data);
                                }
                            }
                            Some(Err(error)) => return Err(LegacyError::Request(error.to_string())),
                            // The reply may still arrive on the standalone stream.
                            None => break,
                        },
                    }
                }
            } else if content_type.contains("application/json") {
                let body = bounded_bytes(response).await?;
                let value: Value = serde_json::from_slice(&body)
                    .map_err(|e| LegacyError::Request(e.to_string()))?;
                self.dispatch(value);
            } else {
                return Err(LegacyError::Request(format!(
                    "Unexpected content type: {content_type}"
                )));
            }
            (&mut rx).await.map_err(|_| LegacyError::Closed)?
        };
        tokio::select! {
            () = self.cancel.cancelled() => Err(LegacyError::Closed),
            reply = work => reply,
        }
    }

    fn dispatch_text(self: &Arc<Self>, data: &str) {
        match serde_json::from_str::<Value>(data) {
            Ok(value) => self.dispatch(value),
            Err(_) => tracing::debug!("ignoring a non-JSON legacy MCP event"),
        }
    }

    fn dispatch(self: &Arc<Self>, message: Value) {
        match message {
            Value::Array(items) => items.into_iter().for_each(|item| self.dispatch(item)),
            Value::Object(mut map) => {
                let id = map.remove("id").filter(|id| !id.is_null());
                if let Some(Value::String(method)) = map.remove("method") {
                    // Notifications need no answer.
                    if let Some(id) = id {
                        let params = match map.remove("params") {
                            Some(Value::Object(params)) => params,
                            _ => Map::new(),
                        };
                        let inner = Arc::clone(self);
                        self.tracker.spawn(async move {
                            tokio::select! {
                                () = inner.cancel.cancelled() => {}
                                () = inner.answer_server_request(id, method, params) => {}
                            }
                        });
                    }
                    return;
                }
                let Some(id) = id.as_ref().and_then(Value::as_i64) else {
                    return;
                };
                let reply = if let Some(error) = map.remove("error") {
                    Err(LegacyError::Rpc {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                } else if let Some(result) = map.remove("result") {
                    Ok(result)
                } else {
                    Err(LegacyError::Request("response without result".into()))
                };
                if let Some(slot) = lock(&self.pending).remove(&id) {
                    let _ = slot.send(reply);
                }
            }
            _ => {}
        }
    }

    async fn answer_server_request(&self, id: Value, method: String, params: Map<String, Value>) {
        let outcome = match (method.as_str(), &self.elicit) {
            ("ping", _) => Ok(json!({})),
            ("elicitation/create", Some(handler)) => {
                match validate_elicitation(&params, self.support) {
                    Err(error) => Err(error),
                    Ok(()) => {
                        let answer = handler(params).await;
                        if valid_answer(&answer) {
                            Ok(answer.to_json())
                        } else {
                            Err((INVALID_PARAMS, "Invalid elicitation result".to_owned()))
                        }
                    }
                }
            }
            _ => Err((METHOD_NOT_FOUND, "Method not found".to_owned())),
        };
        let message = match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        if let Err(error) = self.post(&message).await {
            tracing::warn!(%error, "could not answer an Executor MCP request");
        }
    }

    /// Opens the optional standalone GET stream (a 405 means the server has
    /// none). Waiting briefly for it makes an early
    /// server request less likely to be lost, without letting a server that
    /// delays the stream's headers block the connection.
    async fn open_standalone_stream(self: &Arc<Self>) {
        let (ready_tx, ready_rx) = oneshot::channel();
        let cancel = self.cancel.clone();
        let inner = Arc::clone(self);
        self.tracker.spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = async move {
                    let first = inner.get_stream(None).await;
                    let _ = ready_tx.send(());
                    if let Some(response) = first {
                        inner.run_standalone_stream(response).await;
                    }
                } => {}
            }
        });
        let _ = tokio::time::timeout(STREAM_READY_TIMEOUT, ready_rx).await;
    }

    async fn get_stream(&self, last_event_id: Option<&str>) -> Option<reqwest::Response> {
        let mut request = self
            .http
            .request(Method::GET, self.endpoint.clone())
            .headers(self.headers("text/event-stream"));
        if let Some(id) = last_event_id {
            request = request.header(LAST_EVENT_HEADER, id);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                self.remember_session(&response);
                Some(response)
            }
            Ok(response) if response.status() == StatusCode::METHOD_NOT_ALLOWED => None,
            Ok(response) => {
                tracing::warn!(status = %response.status(), "Executor MCP event stream refused");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "Executor MCP event stream unavailable");
                None
            }
        }
    }

    async fn run_standalone_stream(self: Arc<Self>, first: reqwest::Response) {
        let mut response = first;
        let mut last_event_id = None;
        loop {
            self.read_events(response, &mut last_event_id).await;
            let mut reconnected = None;
            for delay in GET_RECONNECT_DELAYS {
                tokio::time::sleep(delay).await;
                if let Some(next) = self.get_stream(last_event_id.as_deref()).await {
                    reconnected = Some(next);
                    break;
                }
            }
            match reconnected {
                Some(next) => response = next,
                None => return,
            }
        }
    }

    async fn read_events(self: &Arc<Self>, response: reqwest::Response, last: &mut Option<String>) {
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::new(MAX_MESSAGE_BYTES);
        while let Some(Ok(bytes)) = stream.next().await {
            let Ok(events) = decoder.push(&bytes) else {
                return;
            };
            for event in events {
                if event.id.is_some() {
                    last.clone_from(&event.id);
                }
                if event.is_message() {
                    self.dispatch_text(&event.data);
                }
            }
        }
    }
}

/// The SDK's client-side checks before an elicitation reaches the handler.
fn validate_elicitation(
    params: &Map<String, Value>,
    support: ElicitationSupport,
) -> Result<(), (i64, String)> {
    let invalid = || (INVALID_PARAMS, "Invalid elicitation request".to_owned());
    let mode = match params.get("mode") {
        None => "form",
        Some(Value::String(mode)) if mode == "form" || mode == "url" => mode.as_str(),
        Some(_) => return Err(invalid()),
    };
    if !params.get("message").is_some_and(Value::is_string) {
        return Err(invalid());
    }
    if mode == "form" {
        let schema = params.get("requestedSchema").and_then(Value::as_object);
        let well_formed = schema.is_some_and(|schema| {
            schema.get("type").and_then(Value::as_str) == Some("object")
                && schema.get("properties").is_some_and(Value::is_object)
                && schema.get("required").is_none_or(|required| {
                    required
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))
                })
        });
        if !well_formed {
            return Err(invalid());
        }
        if !support.form {
            return Err((
                INVALID_PARAMS,
                "Client does not support form-mode elicitation requests".to_owned(),
            ));
        }
    } else {
        let url = params
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| Url::parse(url).is_ok());
        if !url || !params.get("elicitationId").is_some_and(Value::is_string) {
            return Err(invalid());
        }
        if !support.url {
            return Err((
                INVALID_PARAMS,
                "Client does not support URL-mode elicitation requests".to_owned(),
            ));
        }
    }
    Ok(())
}

/// `ElicitResult.content` is a record of string, number, boolean or string[].
fn valid_answer(answer: &ElicitAnswer) -> bool {
    let ElicitAnswer::Accept(Some(content)) = answer else {
        return true;
    };
    content.values().all(|value| match value {
        Value::String(_) | Value::Number(_) | Value::Bool(_) => true,
        Value::Array(items) => items.iter().all(Value::is_string),
        _ => false,
    })
}

async fn bounded_bytes(response: reqwest::Response) -> Result<Vec<u8>, LegacyError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LegacyError::Request(e.to_string()))?;
        if body.len() + chunk.len() > MAX_MESSAGE_BYTES {
            return Err(LegacyError::Request("response too large".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn bounded_text(response: reqwest::Response) -> String {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        let room = MAX_ERROR_BYTES.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= MAX_ERROR_BYTES {
            break;
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn elicitation_requests_respect_declared_modes() {
        let form = params(json!({
            "message": "m",
            "requestedSchema": {"type": "object", "properties": {}},
        }));
        let url = params(
            json!({"mode": "url", "message": "m", "url": "https://x", "elicitationId": "e"}),
        );
        let both = ElicitationSupport {
            form: true,
            url: true,
        };
        assert!(validate_elicitation(&form, both).is_ok());
        assert!(validate_elicitation(&url, both).is_ok());
        let form_only = ElicitationSupport {
            form: true,
            url: false,
        };
        assert_eq!(
            validate_elicitation(&url, form_only).unwrap_err().1,
            "Client does not support URL-mode elicitation requests"
        );
        let url_only = ElicitationSupport {
            form: false,
            url: true,
        };
        assert_eq!(
            validate_elicitation(&form, url_only).unwrap_err().1,
            "Client does not support form-mode elicitation requests"
        );
        let no_properties = params(json!({"message": "m", "requestedSchema": {"type": "object"}}));
        assert_eq!(
            validate_elicitation(&no_properties, both).unwrap_err().0,
            INVALID_PARAMS
        );
        let no_id = params(json!({"mode": "url", "message": "m", "url": "https://x"}));
        assert_eq!(
            validate_elicitation(&no_id, both).unwrap_err().0,
            INVALID_PARAMS
        );
        let bad_url = params(
            json!({"mode": "url", "message": "m", "url": "not a url", "elicitationId": "e"}),
        );
        assert_eq!(
            validate_elicitation(&bad_url, both).unwrap_err().0,
            INVALID_PARAMS
        );
        let bad = params(json!({"mode": "audio", "message": "m"}));
        assert_eq!(
            validate_elicitation(&bad, both).unwrap_err().0,
            INVALID_PARAMS
        );
    }

    #[test]
    fn answers_must_carry_primitive_content() {
        assert!(valid_answer(&ElicitAnswer::Decline));
        assert!(valid_answer(&ElicitAnswer::Accept(None)));
        let content = params(json!({"a": "x", "b": 1, "c": true, "d": ["x"]}));
        assert!(valid_answer(&ElicitAnswer::Accept(Some(content))));
        assert!(!valid_answer(&ElicitAnswer::Accept(Some(params(
            json!({"a": {"b": 1}})
        )))));
        assert!(!valid_answer(&ElicitAnswer::Accept(Some(params(
            json!({"a": [1]})
        )))));
    }
}
