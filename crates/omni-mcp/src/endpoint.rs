//! `ALL /mcp`.
//!
//! 503 when no MCP token is configured; otherwise a constant-time bearer
//! check (SHA-256 digests) answers 401 before any MCP handling, and every MCP
//! response carries `Cache-Control: no-store` and `X-Content-Type-Options:
//! nosniff`.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::routing::any;
use serde_json::{Value, json};

use crate::rpc::{MAX_REQUEST_BYTES, McpProtocol};

/// A JSON body serialized with JS `JSON.stringify` semantics.
pub fn json_response(status: StatusCode, value: &Value) -> Response {
    let mut response = Response::new(Body::from(omni_core::js::json_stringify(value)));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

pub fn unauthorized() -> Response {
    let mut response = json_response(StatusCode::UNAUTHORIZED, &json!({"error": "Unauthorized"}));
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

#[derive(Clone)]
struct EndpointState {
    served: Option<(Arc<str>, McpProtocol)>,
}

/// The `/mcp` route; `None` serves the "not configured" 503.
pub fn router(token: Option<&str>, protocol: Option<McpProtocol>) -> Router {
    let served = token
        .filter(|token| !token.is_empty())
        .zip(protocol)
        .map(|(token, protocol)| (Arc::from(token), protocol));
    Router::new()
        .route("/mcp", any(handle))
        .with_state(EndpointState { served })
}

async fn handle(
    State(state): State<EndpointState>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some((token, protocol)) = &state.served else {
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error": "MCP is not configured"}),
        );
    };
    if !omni_server_kit::bearer_digest_eq(headers.get(AUTHORIZATION), token) {
        return unauthorized();
    }
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let bytes = match declared {
        Some(length) if length > MAX_REQUEST_BYTES as u64 => None,
        _ => axum::body::to_bytes(body, MAX_REQUEST_BYTES).await.ok(),
    };
    let mut response = match bytes {
        Some(bytes) => protocol.serve(&method, &headers, bytes).await,
        None => json_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            &json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "Payload too large"}, "id": null}),
        ),
    };
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}
