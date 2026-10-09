//! Router composition: SPA fallback and cache headers, JSON 404 for unknown
//! `/api/*`, the same-origin guard on composed routes, and the opaque 500 for
//! handler failures and panics.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use omni_server_kit::{ApiError, ApiResult, app_router};
use tower::ServiceExt as _;

async fn call(router: Router, request: Request) -> (Response, String) {
    let response = router.oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    (
        Response::from_parts(parts, Body::empty()),
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn get_req(uri: &str) -> Request {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn parts() -> Router {
    Router::new()
        .route(
            "/api/ok",
            get(|| async { Ok::<_, ApiError>(axum::Json(serde_json::json!({ "ok": true }))) }),
        )
        .route(
            "/api/missing",
            get(|| async { Err::<axum::Json<()>, _>(ApiError::not_found("Unknown run")) }),
        )
        .route(
            "/api/broken",
            get(|| async { Err::<axum::Json<()>, _>(ApiError::internal("disk full")) }),
        )
        .route(
            "/api/panic",
            get(|| async {
                let result: ApiResult<()> = Err(ApiError::internal("unused"));
                if result.is_err() {
                    panic!("handler bug");
                }
                result
            }),
        )
        .route("/api/mutate", post(|| async { "done" }))
}

#[tokio::test]
async fn api_errors_and_panics_render_like_hono() {
    let app = app_router([parts()], None);
    let (ok, body) = call(app.clone(), get_req("/api/ok")).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(body, r#"{"ok":true}"#);
    assert_eq!(ok.headers()["x-content-type-options"], "nosniff");

    let (missing, body) = call(app.clone(), get_req("/api/missing")).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"Unknown run"}"#);

    for path in ["/api/broken", "/api/panic"] {
        let (failed, body) = call(app.clone(), get_req(path)).await;
        assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        assert_eq!(body, "Internal Server Error");
        assert!(
            failed.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/plain")
        );
    }

    let (unknown, body) = call(app, get_req("/api/nope")).await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"Not found"}"#);
}

#[tokio::test]
async fn composed_routes_get_the_same_origin_guard() {
    let app = app_router([parts()], None);
    let request = |origin: &str| {
        Request::builder()
            .method("POST")
            .uri("/api/mutate")
            .header("host", "omni.boris")
            .header("origin", origin)
            .body(Body::empty())
            .unwrap()
    };
    let (allowed, _) = call(app.clone(), request("http://omni.boris")).await;
    assert_eq!(allowed.status(), StatusCode::OK);
    let (blocked, body) = call(app, request("https://evil.example")).await;
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body,
        r#"{"error":"Cross-origin mutations are not allowed"}"#
    );
}

#[tokio::test]
async fn serves_the_spa_with_cache_rules_and_index_fallback() {
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(
        dist.path().join("index.html"),
        "<!doctype html><title>Omni</title>",
    )
    .unwrap();
    std::fs::create_dir(dist.path().join("assets")).unwrap();
    std::fs::write(dist.path().join("assets/app-abc123.js"), "console.log(1)").unwrap();
    let app = app_router([parts()], Some(dist.path().to_owned()));

    let (asset, body) = call(app.clone(), get_req("/assets/app-abc123.js")).await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(body, "console.log(1)");
    assert_eq!(
        asset.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );

    for route in ["/", "/streamers/abc/intelligence", "/pods"] {
        let (page, body) = call(app.clone(), get_req(route)).await;
        assert_eq!(page.status(), StatusCode::OK, "{route}");
        assert!(body.contains("<title>Omni</title>"));
        assert_eq!(page.headers()["cache-control"], "no-cache");
    }

    let (unknown_api, body) = call(app.clone(), get_req("/api/nope")).await;
    assert_eq!(unknown_api.status(), StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"Not found"}"#);

    let (concrete, _) = call(app, get_req("/api/ok")).await;
    assert_eq!(concrete.status(), StatusCode::OK);
}
