//! Reminders admin routes and the `/reminders` page headers. The typed status
//! cannot carry arbitrary diagnostic labels or secret fields, so only bounded
//! HTTP statuses are checked.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::type_complexity)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::future::BoxFuture;
use omni_api::reminders::{
    CodeRequest, Diagnostic, DiagnosticCategory, DiagnosticStage, Phase, PublicStatus,
};
use omni_reminders::routes::{RemindersControl, reminders_router};
use serde_json::{Value, json};
use tower::ServiceExt as _;

const ORIGIN: &str = "https://omni.example.test";

fn initial() -> PublicStatus {
    PublicStatus::new(true, Phase::AuthenticationNeeded)
}

type Reply = Box<dyn Fn() -> Result<PublicStatus, ()> + Send + Sync>;

struct Fake {
    status: Mutex<Reply>,
    start: Mutex<Reply>,
    verify: Mutex<Reply>,
    status_calls: AtomicUsize,
    start_calls: AtomicUsize,
    codes: Mutex<Vec<CodeRequest>>,
}

impl RemindersControl for Fake {
    fn status(&self) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        let r = (self.status.lock().unwrap())();
        Box::pin(async move { r })
    }

    fn start_authentication(&self) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        let r = (self.start.lock().unwrap())();
        Box::pin(async move { r })
    }

    fn submit_code(&self, input: CodeRequest) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        self.codes.lock().unwrap().push(input);
        Box::pin(async { Ok(PublicStatus::new(true, Phase::Authenticated)) })
    }

    fn verify_access(&self) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        let r = (self.verify.lock().unwrap())();
        Box::pin(async move { r })
    }
}

struct Fixture {
    router: Router,
    fake: Arc<Fake>,
}

fn fixture(origin: &str) -> Fixture {
    let fake = Arc::new(Fake {
        status: Mutex::new(Box::new(|| Ok(initial()))),
        start: Mutex::new(Box::new(|| {
            Ok(PublicStatus {
                challenge_id: Some("challenge-one".into()),
                ..initial()
            })
        })),
        verify: Mutex::new(Box::new(|| Ok(initial()))),
        status_calls: AtomicUsize::new(0),
        start_calls: AtomicUsize::new(0),
        codes: Mutex::new(Vec::new()),
    });
    let router = reminders_router(
        fake.clone(),
        Some(origin),
        omni_testkit::test_clock(1_800_000_000_000),
    );
    Fixture { router, fake }
}

struct Response {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    text: String,
}

impl Response {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

async fn request(
    router: &Router,
    path: &str,
    method: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "omni.example.test");
    if method != "GET" {
        builder = builder
            .header("origin", ORIGIN)
            .header("content-type", "application/json");
    }
    let mut request = builder.body(Body::from(body.to_owned())).unwrap();
    for (name, value) in headers {
        let name = axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap();
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    Response {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

#[tokio::test]
async fn includes_safe_diagnostic_status_in_failed_start_and_verify_responses() {
    let x = fixture(ORIGIN);
    let diagnostic = Diagnostic {
        stage: DiagnosticStage::SignInInit,
        category: DiagnosticCategory::AppleResponse,
        http_status: Some(503),
    };
    *x.fake.status.lock().unwrap() = Box::new(move || {
        Ok(PublicStatus {
            diagnostic: Some(diagnostic),
            ..PublicStatus::new(true, Phase::TransientOutage)
        })
    });
    *x.fake.start.lock().unwrap() = Box::new(|| Err(()));
    *x.fake.verify.lock().unwrap() = Box::new(|| Err(()));
    for action in ["start", "verify"] {
        let response = request(
            &x.router,
            &format!("/api/reminders/auth/{action}"),
            "POST",
            &[],
            "{}",
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            response.json(),
            json!({
                "error": "Reminders request failed",
                "status": {
                    "enabled": true,
                    "phase": "transient-outage",
                    "diagnostic": {"stage": "sign-in-init", "category": "apple-response", "httpStatus": 503},
                },
            })
        );
    }
}

#[tokio::test]
async fn rejects_invalid_status_codes_in_diagnostics() {
    let x = fixture(ORIGIN);
    *x.fake.status.lock().unwrap() = Box::new(|| {
        Ok(PublicStatus {
            diagnostic: Some(Diagnostic {
                stage: DiagnosticStage::SignInInit,
                category: DiagnosticCategory::Transport,
                http_status: Some(900),
            }),
            ..initial()
        })
    });
    let response = request(&x.router, "/api/reminders/status", "GET", &[], "").await;
    assert_eq!(
        response.json(),
        json!({"status": {"enabled": true, "phase": "authentication-needed", "diagnostic": {"stage": "sign-in-init", "category": "transport"}}})
    );
}

#[tokio::test]
async fn returns_429_when_authentication_is_in_its_service_cooldown() {
    let x = fixture(ORIGIN);
    *x.fake.status.lock().unwrap() = Box::new(|| Ok(PublicStatus::new(true, Phase::RateLimited)));
    *x.fake.start.lock().unwrap() = Box::new(|| Err(()));
    let response = request(&x.router, "/api/reminders/auth/start", "POST", &[], "{}").await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.json(),
        json!({"error": "Reminders request failed", "status": {"enabled": true, "phase": "rate-limited"}})
    );
}

#[tokio::test]
async fn returns_only_public_status_metadata_with_no_store_headers() {
    let x = fixture(ORIGIN);
    let response = request(&x.router, "/api/reminders/status", "GET", &[], "").await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["cache-control"], "no-store");
    assert_eq!(response.headers["pragma"], "no-cache");
    assert_eq!(response.headers["x-content-type-options"], "nosniff");
    assert_eq!(
        response.json(),
        json!({"status": {"enabled": true, "phase": "authentication-needed"}})
    );
}

#[tokio::test]
async fn rejects_mutations_with_missing_or_mismatched_origin() {
    let x = fixture(ORIGIN);
    for origin in ["", "https://evil.example.test", "http://omni.example.test"] {
        let response = request(
            &x.router,
            "/api/reminders/auth/start",
            "POST",
            &[("origin", origin)],
            "{}",
        )
        .await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{origin:?}");
    }
    assert_eq!(x.fake.start_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_cross_site_fetch_metadata() {
    let x = fixture(ORIGIN);
    let response = request(
        &x.router,
        "/api/reminders/auth/start",
        "POST",
        &[("sec-fetch-site", "cross-site")],
        "{}",
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(x.fake.start_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_a_mismatched_host_and_insecure_configuration() {
    let x = fixture(ORIGIN);
    let response = request(
        &x.router,
        "/api/reminders/status",
        "GET",
        &[("host", "evil.example.test")],
        "",
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let insecure = fixture("http://omni.example.test");
    let status = request(&insecure.router, "/api/reminders/status", "GET", &[], "").await;
    assert_eq!(
        status.json(),
        json!({"status": {"enabled": false, "phase": "disabled", "reason": "configuration"}})
    );
    let start = request(
        &insecure.router,
        "/api/reminders/auth/start",
        "POST",
        &[],
        "{}",
    )
    .await;
    assert_eq!(start.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(insecure.fake.status_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bounds_and_validates_the_verification_code_before_calling_the_service() {
    let x = fixture(ORIGIN);
    let malformed = request(
        &x.router,
        "/api/reminders/auth/code",
        "POST",
        &[],
        &json!({"challengeId": "challenge-one", "code": "1234567"}).to_string(),
    )
    .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert!(x.fake.codes.lock().unwrap().is_empty());
    let oversized = request(
        &x.router,
        "/api/reminders/auth/code",
        "POST",
        &[],
        &json!({"challengeId": "c".repeat(600), "code": "123456"}).to_string(),
    )
    .await;
    assert_eq!(oversized.status, StatusCode::BAD_REQUEST);
    let valid = request(
        &x.router,
        "/api/reminders/auth/code",
        "POST",
        &[],
        &json!({"challengeId": "challenge-one", "code": "123456"}).to_string(),
    )
    .await;
    assert_eq!(valid.status, StatusCode::OK);
    assert_eq!(
        *x.fake.codes.lock().unwrap(),
        vec![CodeRequest {
            challenge_id: "challenge-one".into(),
            code: "123456".into()
        }]
    );
}

#[tokio::test]
async fn requires_json_content_type_for_mutations() {
    let x = fixture(ORIGIN);
    let response = request(
        &x.router,
        "/api/reminders/auth/verify",
        "POST",
        &[("content-type", "text/plain")],
        "{}",
    )
    .await;
    assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let charset = request(
        &x.router,
        "/api/reminders/auth/verify",
        "POST",
        &[("content-type", "application/json; charset=utf-8")],
        "{}",
    )
    .await;
    assert_eq!(charset.status, StatusCode::OK);
}

#[tokio::test]
async fn does_not_return_service_errors_to_the_client() {
    let x = fixture(ORIGIN);
    *x.fake.start.lock().unwrap() = Box::new(|| Err(()));
    *x.fake.status.lock().unwrap() = Box::new(|| Err(()));
    let response = request(&x.router, "/api/reminders/auth/start", "POST", &[], "{}").await;
    assert_eq!(response.status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.json(),
        json!({"error": "Reminders request failed"})
    );
}

#[tokio::test]
async fn bounds_repeated_authentication_starts() {
    let x = fixture(ORIGIN);
    for _ in 0..3 {
        assert_eq!(
            request(&x.router, "/api/reminders/auth/start", "POST", &[], "{}")
                .await
                .status,
            StatusCode::OK
        );
    }
    let limited = request(&x.router, "/api/reminders/auth/start", "POST", &[], "{}").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers["cache-control"], "no-store");
    assert_eq!(limited.headers["retry-after"], "300");
    assert_eq!(x.fake.start_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn serves_the_code_page_with_a_strict_csp_and_no_extra_login() {
    let router = omni_reminders::page::page_router();
    for (path, content_type) in [
        ("/reminders", "text/html"),
        ("/reminders/app.js", "text/javascript"),
    ] {
        let response = request(&router, path, "GET", &[], "").await;
        assert_eq!(response.status, StatusCode::OK, "{path}");
        assert!(
            response.headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with(content_type)
        );
        assert_eq!(response.headers["cache-control"], "no-store");
        assert_eq!(response.headers["referrer-policy"], "no-referrer");
        let csp = response.headers["content-security-policy"]
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self';"), "{csp}");
        assert!(!csp.contains("wasm-unsafe-eval"));
    }
    let html = request(&router, "/reminders", "GET", &[], "").await.text;
    assert!(html.contains(r#"<script src="/reminders/app.js"></script>"#));
    assert!(
        !html.contains("<script>"),
        "inline scripts would violate the CSP"
    );
    assert!(!html.to_lowercase().contains("password"));
}
