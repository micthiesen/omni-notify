//! Provider wire clients. Each module exposes a pure `request_body` builder and a pure
//! `parse_response`, plus a [`LanguageModel`] that posts one non-streaming request.
//!
//! Requests go through `omni-http` (project user agent, no automatic redirects); the
//! per-call timeout and retries live in [`crate::call_with_retries`].

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_config::ModelRole;
use omni_http::{HttpClient, Method, Url};
use serde_json::Value;

use crate::{AiError, GenerateRequest, GenerateResponse, LanguageModel, ModelId, Provider};

pub mod anthropic_messages;
pub mod gemini;
pub mod openai_responses;

/// Largest provider response body read (structured outputs are far smaller).
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

pub const OPENAI_BASE: &str = "https://api.openai.com/v1";
pub const ANTHROPIC_BASE: &str = "https://api.anthropic.com/v1";
pub const GOOGLE_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// The wire client for `id` (tests redirect the base URLs with `HttpOverrides`).
pub fn client(
    http: HttpClient,
    id: ModelId,
    key: String,
    role: Option<ModelRole>,
) -> Arc<dyn LanguageModel> {
    Arc::new(WireModel {
        http,
        id,
        key,
        role,
    })
}

struct WireModel {
    http: HttpClient,
    id: ModelId,
    key: String,
    role: Option<ModelRole>,
}

impl LanguageModel for WireModel {
    fn id(&self) -> &ModelId {
        &self.id
    }

    fn role(&self) -> Option<ModelRole> {
        self.role
    }

    fn generate<'a>(
        &'a self,
        req: &'a GenerateRequest,
    ) -> BoxFuture<'a, Result<GenerateResponse, AiError>> {
        Box::pin(async move {
            let model = self.id.model.as_str();
            let (url, body) = match self.id.provider {
                Provider::OpenAi => (
                    format!("{OPENAI_BASE}/responses"),
                    openai_responses::request_body(model, req),
                ),
                Provider::Anthropic => (
                    format!("{ANTHROPIC_BASE}/messages"),
                    anthropic_messages::request_body(model, req),
                ),
                Provider::Google => (
                    format!(
                        "{GOOGLE_BASE}/models/{}:generateContent",
                        omni_core::js::encode_uri_component(model)
                    ),
                    gemini::request_body(req),
                ),
            };
            let url = Url::parse(&url).map_err(|e| AiError::Schema(e.to_string()))?;
            let request = self.http.request(Method::POST, url).json(&body);
            let request = match self.id.provider {
                Provider::OpenAi => request.bearer_auth(&self.key),
                Provider::Anthropic => request
                    .header("x-api-key", self.key.as_str())
                    .header("anthropic-version", anthropic_messages::API_VERSION),
                Provider::Google => request.header("x-goog-api-key", self.key.as_str()),
            };
            let response = request.send_bounded(MAX_RESPONSE_BYTES).await?;
            if !response.status.is_success() {
                return Err(provider_error(response.status.as_u16(), &response.body));
            }
            match self.id.provider {
                Provider::OpenAi => openai_responses::parse_response(&response.body),
                Provider::Anthropic => anthropic_messages::parse_response(&response.body),
                Provider::Google => gemini::parse_response(&response.body),
            }
        })
    }
}

/// `{"error": {"message": ...}}` (all three providers), else the body text.
pub fn provider_error(status: u16, body: &[u8]) -> AiError {
    let message = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body);
            omni_core::js::utf16_slice(&text, 0, 4096).into_owned()
        });
    AiError::Provider { status, message }
}

pub(crate) fn decode_json(body: &[u8]) -> Result<Value, AiError> {
    serde_json::from_slice(body)
        .map_err(|e| AiError::Schema(format!("invalid provider response JSON: {e}")))
}

pub(crate) fn data_url(mime_type: &str, data_b64: &str) -> String {
    format!("data:{mime_type};base64,{data_b64}")
}

/// Tool output encoding: strings are sent as-is, everything else as JSON.
pub(crate) fn tool_output_text(output: &Value) -> String {
    match output {
        Value::String(text) => text.clone(),
        other => omni_core::js::json_stringify(other),
    }
}

/// Tool-call arguments: parsed JSON, or the raw string when the model produced
/// invalid JSON (the tool then rejects it and the model sees the error).
pub(crate) fn parse_arguments(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

pub(crate) fn u64_at(value: &Value, pointer: &str) -> u64 {
    value.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

pub(crate) fn join_nonempty(parts: Vec<String>) -> Option<String> {
    let parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}
