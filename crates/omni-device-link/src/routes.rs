//! Host-facing long-poll endpoints, authenticated
//! by `OMNI_DEVICE_LINK_TOKEN` only.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use omni_core::js::utf16_len;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::service::{DeviceJobOutcome, DeviceLinkService, PollReport};

/// `MAX_JSON_BODY_BYTES` for polls.
pub const DEVICE_POLL_MAX_BYTES: usize = 64 * 1024;
/// Results carry session transcripts, so they get a larger cap.
pub const DEVICE_RESULT_MAX_BYTES: usize = 512 * 1024;

#[derive(Clone)]
struct RouteState {
    service: DeviceLinkService,
    token: Arc<str>,
}

/// `POST /device-link/poll` and `POST /device-link/result`.
pub fn router(service: DeviceLinkService, token: &str) -> Router {
    Router::new()
        .route("/device-link/poll", post(poll))
        .route("/device-link/result", post(result))
        .with_state(RouteState {
            service,
            token: Arc::from(token),
        })
}

/// `unauthorizedMcpResponse()`.
pub fn unauthorized() -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        axum::Json(json!({"error": "Unauthorized"})),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn bad_request() -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({"error": "Bad request"})),
    )
        .into_response()
}

fn no_store(body: Value) -> Response {
    let mut response = axum::Json(body).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `readJsonBody`: a declared length over the cap or an oversized body fails.
async fn read_json(headers: &HeaderMap, body: Body, max: usize) -> Option<Value> {
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|len| len > max as u64) {
        return None;
    }
    let bytes = axum::body::to_bytes(body, max).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn max_len(value: &str, max: usize) -> bool {
    utf16_len(value) <= max
}

#[derive(Deserialize)]
struct PollBody {
    v: u8,
    disabled: bool,
    #[serde(default)]
    host: Option<String>,
    #[serde(default, rename = "agentVersion")]
    agent_version: Option<String>,
}

impl PollBody {
    fn valid(&self) -> bool {
        self.v == 1
            && self.host.as_deref().is_none_or(|host| max_len(host, 200))
            && self
                .agent_version
                .as_deref()
                .is_none_or(|version| max_len(version, 40))
    }
}

#[derive(Deserialize)]
struct ResultError {
    code: String,
    message: String,
}

#[derive(Deserialize)]
struct ResultBody {
    v: u8,
    id: String,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    error: Option<ResultError>,
}

impl ResultBody {
    fn valid(&self) -> bool {
        self.v == 1
            && !self.id.is_empty()
            && max_len(&self.id, 100)
            && self
                .error
                .as_ref()
                .is_none_or(|e| max_len(&e.code, 100) && max_len(&e.message, 4_000))
    }
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    omni_server_kit::bearer_digest_eq(headers.get(axum::http::header::AUTHORIZATION), token)
}

/// Holds the poll; when the host disconnects, hyper drops this future and the
/// held poll stops claiming jobs at its next await.
async fn poll(State(state): State<RouteState>, headers: HeaderMap, body: Body) -> Response {
    if !authorized(&headers, &state.token) {
        return unauthorized();
    }
    let Some(parsed) = read_json(&headers, body, DEVICE_POLL_MAX_BYTES)
        .await
        .and_then(|value| serde_json::from_value::<PollBody>(value).ok())
        .filter(PollBody::valid)
    else {
        return bad_request();
    };
    let jobs = state
        .service
        .poll(PollReport {
            disabled: parsed.disabled,
            host: parsed.host,
        })
        .await;
    match serde_json::to_value(jobs) {
        Ok(jobs) => no_store(json!({"v": 1, "jobs": jobs})),
        Err(_) => bad_request(),
    }
}

async fn result(State(state): State<RouteState>, headers: HeaderMap, body: Body) -> Response {
    if !authorized(&headers, &state.token) {
        return unauthorized();
    }
    let Some(parsed) = read_json(&headers, body, DEVICE_RESULT_MAX_BYTES)
        .await
        .and_then(|value| serde_json::from_value::<ResultBody>(value).ok())
        .filter(ResultBody::valid)
    else {
        return bad_request();
    };
    let outcome = match parsed.error {
        Some(error) => DeviceJobOutcome::Error {
            code: error.code,
            message: error.message,
        },
        None => DeviceJobOutcome::Output(parsed.output.unwrap_or(Value::Null)),
    };
    let accepted = state.service.complete(&parsed.id, outcome);
    no_store(json!({"v": 1, "accepted": accepted}))
}
