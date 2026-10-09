//! Bounded JSON request bodies. An oversize body is rejected by the extractor
//! itself, which answers 413 before the handler runs.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::post;
use bytes::Bytes;
use omni_server_kit::{JsonBodyLimit, app_router};
use tower::ServiceExt as _;

const TEST_BODY_LIMIT: usize = 32;

#[derive(serde::Deserialize, serde::Serialize)]
struct Payload {
    value: String,
}

fn app() -> Router {
    app_router(
        [Router::new().route(
            "/api/json",
            post(
                |JsonBodyLimit(body): JsonBodyLimit<TEST_BODY_LIMIT, Payload>| async move {
                    axum::Json(body)
                },
            ),
        )],
        None,
    )
}

async fn send(body: Body, content_length: Option<usize>) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/json")
        .header("content-type", "application/json");
    if let Some(length) = content_length {
        builder = builder.header("content-length", length.to_string());
    }
    let response = app().oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn decodes_a_valid_body_under_the_limit() {
    let (status, body) = send(Body::from(r#"{"value":"ok"}"#), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "value": "ok" }));
}

/// A streamed body that records whether it was read to the end.
fn tracked_stream(chunks: Vec<Vec<u8>>, finished: Arc<AtomicBool>) -> Body {
    let stream = futures::stream::iter(
        chunks
            .into_iter()
            .map(|chunk| Ok::<_, std::io::Error>(Bytes::from(chunk)))
            .chain(std::iter::from_fn(move || {
                finished.store(true, Ordering::SeqCst);
                None
            })),
    );
    Body::from_stream(stream)
}

#[tokio::test]
async fn rejects_an_oversized_declared_length_without_reading() {
    let finished = Arc::new(AtomicBool::new(false));
    let (status, body) = send(
        tracked_stream(vec![b"{}".to_vec()], finished.clone()),
        Some(TEST_BODY_LIMIT + 1),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body,
        serde_json::json!({ "error": "Request body too large" })
    );
    assert!(!finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn rejects_a_chunked_body_once_it_crosses_the_limit() {
    let finished = Arc::new(AtomicBool::new(false));
    let (status, body) = send(
        tracked_stream(
            vec![vec![b' '; TEST_BODY_LIMIT], vec![b' '], vec![b' '; 1024]],
            finished.clone(),
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body,
        serde_json::json!({ "error": "Request body too large" })
    );
    assert!(
        !finished.load(Ordering::SeqCst),
        "the stream was read past the limit"
    );
}

#[tokio::test]
async fn invalid_json_or_shape_is_a_bad_request() {
    let (status, body) = send(Body::from("not json"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].is_string());
    let (status, _) = send(Body::from(r#"{"value":1}"#), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
