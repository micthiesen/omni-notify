//! Parcel subsystem wiring and the delivery-forget route.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use omni_api::email::paths;
use omni_config::Config;
use omni_email::triage::EmailTriage;
use omni_parcel::persistence::{self, DeliveryAttempt, SubmissionStatus};
use omni_testkit::{TestApp, test_app_env};
use serde_json::{Value, json};
use tower::ServiceExt as _;

fn triage(app: &TestApp) -> EmailTriage {
    EmailTriage::with_model(
        app.ctx.ai.clone(),
        app.ctx.config.clone(),
        app.ctx.store.clone(),
    )
}

async fn delete(router: &axum::Router, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::DELETE)
        .uri(path)
        .header(header::HOST, "localhost")
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn is_disabled_without_an_api_key_and_registers_the_handler_with_one() {
    let app = TestApp::new().await;
    let without = omni_parcel::subsystem(&app.ctx, triage(&app)).unwrap();
    assert!(without.email_handlers.is_empty());
    assert_eq!(without.entities[0].name, "parcel-submitted-delivery");

    let mut env = test_app_env();
    env.insert("PARCEL_API_KEY".to_owned(), "parcel-key".to_owned());
    let mut ctx = app.ctx.clone();
    ctx.config = Arc::new(Config::from_env(&env).unwrap());
    let with = omni_parcel::subsystem(&ctx, triage(&app)).unwrap();
    assert_eq!(with.email_handlers.len(), 1);
    assert_eq!(with.email_handlers[0].name(), "ParcelTracker");
}

#[tokio::test]
async fn forgets_a_submitted_delivery() {
    let app = TestApp::new().await;
    let subsystem = omni_parcel::subsystem(&app.ctx, triage(&app)).unwrap();
    let router = app.router(&subsystem);
    persistence::record(
        &app.ctx.store,
        DeliveryAttempt {
            tracking_number: "1Z 99/A".to_owned(),
            carrier_code: "ups".to_owned(),
            description: "Camera".to_owned(),
            submitted_at: 1,
            email_id: "m1".to_owned(),
        },
        SubmissionStatus::Submitted,
        Some(1),
    )
    .await
    .unwrap();
    let path = paths::parcel_delivery("1Z 99/A");
    assert_eq!(
        delete(&router, &path).await,
        (StatusCode::OK, json!({"deleted": true}))
    );
    assert_eq!(
        delete(&router, &path).await,
        (
            StatusCode::NOT_FOUND,
            json!({"error": "Unknown tracking number"})
        )
    );
}
