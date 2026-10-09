//! Executor's legacy (pre-2026) MCP, reached as a Streamable HTTP client.

mod client;
pub mod sse;

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Map, Value, json};

pub use client::{HttpConnector, LEGACY_PROTOCOL_VERSIONS};

/// Default per-request timeout (the MCP SDK's default).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Connection (initialize handshake) deadline.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on closing a legacy client.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Elicitation modes the modern caller advertised; the legacy client declares the
/// same capability to Executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ElicitationSupport {
    pub form: bool,
    pub url: bool,
}

impl ElicitationSupport {
    pub fn any(self) -> bool {
        self.form || self.url
    }

    /// `{elicitation: {form?: {}, url?: {}}}` or `{}`.
    pub fn capabilities(self) -> Value {
        if !self.any() {
            return json!({});
        }
        let mut elicitation = Map::new();
        if self.form {
            elicitation.insert("form".into(), json!({}));
        }
        if self.url {
            elicitation.insert("url".into(), json!({}));
        }
        json!({ "elicitation": elicitation })
    }
}

/// A user's answer to an `elicitation/create` request.
#[derive(Debug, Clone, PartialEq)]
pub enum ElicitAnswer {
    Accept(Option<Map<String, Value>>),
    Decline,
    Cancel,
}

impl ElicitAnswer {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Accept(Some(content)) => json!({"action": "accept", "content": content}),
            Self::Accept(None) => json!({"action": "accept"}),
            Self::Decline => json!({"action": "decline"}),
            Self::Cancel => json!({"action": "cancel"}),
        }
    }
}

/// Receives Executor's `elicitation/create` params and resolves with the answer.
pub type ElicitHandler =
    Arc<dyn Fn(Map<String, Value>) -> BoxFuture<'static, ElicitAnswer> + Send + Sync>;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum LegacyError {
    #[error("Executor MCP connection failed: {0}")]
    Connect(String),
    #[error("Executor MCP request failed: {0}")]
    Request(String),
    #[error("Executor MCP error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("Executor MCP request timed out")]
    Timeout,
    #[error("Executor MCP client closed")]
    Closed,
}

/// One connected legacy MCP session.
pub trait LegacySession: Send + Sync {
    /// Sends a request and resolves with its `result`.
    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<Value, LegacyError>>;

    /// Stops all streams and in-flight work. Idempotent.
    fn close(&self) -> BoxFuture<'_, ()>;
}

/// Opens legacy sessions. The seam the continuation tests replace.
pub trait LegacyConnector: Send + Sync {
    /// `mode` is the caller's `elicitation_mode` query value; only `native`,
    /// `browser` and `model` reach Executor. `elicit` is `Some` exactly when
    /// `support.any()`.
    fn connect(
        &self,
        authorization: String,
        mode: Option<String>,
        support: ElicitationSupport,
        elicit: Option<ElicitHandler>,
    ) -> BoxFuture<'static, Result<Arc<dyn LegacySession>, LegacyError>>;
}
