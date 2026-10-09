//! Port of `src/device-link/routes.spec.ts`.
//!
//! "stops holding a poll when the Mac disconnects": Hono answers an aborted
//! request with `{v:1, jobs:[]}`; hyper instead drops the handler future when
//! the client goes away, so there is no response to read. The Rust case drops
//! the in-flight request and checks what matters: the poll stops holding (no
//! job can be claimed by it) and the reported kill-switch state was recorded.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use omni_core::clock::TestClock;
use omni_device_link::{DeviceCommand, DeviceLinkService, routes};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

const TOKEN: &str = "device-token-that-is-definitely-long-enough";

fn setup() -> (Router, DeviceLinkService) {
    let link = DeviceLinkService::new(TestClock::new(1_790_000_000_000));
    (routes::router(link.clone(), TOKEN), link)
}

fn request(path: &str, body: &Value, auth: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {auth}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value, Option<String>) {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let cache = response
        .headers()
        .get("cache-control")
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        cache,
    )
}

#[tokio::test]
async fn rejects_requests_without_the_device_token() {
    let (app, _) = setup();
    let (status, body, cache) = send(
        &app,
        request(
            "/device-link/poll",
            &json!({"v": 1, "disabled": true}),
            "wrong",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({"error": "Unauthorized"}));
    assert_eq!(cache.as_deref(), Some("no-store"));
    let (status, _, _) = send(
        &app,
        request("/device-link/result", &json!({"v": 1, "id": "x"}), ""),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_malformed_bodies() {
    let (app, _) = setup();
    let (status, body, _) = send(
        &app,
        request(
            "/device-link/poll",
            &json!({"v": 2, "disabled": false}),
            TOKEN,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({"error": "Bad request"}));
    let (status, _, _) = send(
        &app,
        request("/device-link/result", &json!({"v": 1}), TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let long_host = "h".repeat(201);
    let (status, _, _) = send(
        &app,
        request(
            "/device-link/poll",
            &json!({"v": 1, "disabled": false, "host": long_host}),
            TOKEN,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(start_paused = true)]
async fn relays_a_job_to_the_polling_mac_and_its_result_back_to_the_caller() {
    let (app, link) = setup();
    let polling = {
        let app = app.clone();
        tokio::spawn(async move {
            send(
                &app,
                request(
                    "/device-link/poll",
                    &json!({"v": 1, "disabled": false, "host": "MaxBook"}),
                    TOKEN,
                ),
            )
            .await
        })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let call = {
        let link = link.clone();
        tokio::spawn(async move {
            link.execute(DeviceCommand::Projects, Map::new(), Duration::from_secs(5))
                .await
        })
    };
    let (status, body, cache) = polling.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache.as_deref(), Some("no-store"));
    let jobs = body["jobs"].as_array().unwrap().clone();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["command"], "projects");
    assert_eq!(jobs[0]["args"], json!({}));
    let id = jobs[0]["id"].as_str().unwrap().to_owned();

    let output = json!({"v": 1, "ok": true, "data": {"projects": []}});
    let (_, accepted, _) = send(
        &app,
        request(
            "/device-link/result",
            &json!({"v": 1, "id": id, "output": output}),
            TOKEN,
        ),
    )
    .await;
    assert_eq!(accepted, json!({"v": 1, "accepted": true}));
    assert_eq!(
        Value::Object(call.await.unwrap().unwrap()),
        json!({"projects": []})
    );
    let (_, again, _) = send(
        &app,
        request(
            "/device-link/result",
            &json!({"v": 1, "id": id, "output": output}),
            TOKEN,
        ),
    )
    .await;
    assert_eq!(again, json!({"v": 1, "accepted": false}));
}

#[tokio::test(start_paused = true)]
async fn stops_holding_a_poll_when_the_mac_disconnects() {
    let (app, link) = setup();
    let polling = {
        let app = app.clone();
        tokio::spawn(async move {
            send(
                &app,
                request(
                    "/device-link/poll",
                    &json!({"v": 1, "disabled": true}),
                    TOKEN,
                ),
            )
            .await
        })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    polling.abort();
    assert!(polling.await.unwrap_err().is_cancelled());
    assert!(link.status().disabled);
}
