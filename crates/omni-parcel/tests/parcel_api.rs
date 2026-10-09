//! The Parcel `add-delivery` adapter against a local mock (live mode), and
//! `SideEffectMode::Record`, which must never send.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_core::clock::TestClock;
use omni_http::SideEffectMode;
use omni_parcel::log_file::{LogFile, LogFileMode};
use omni_parcel::parcel_api::{ParcelApi, ParcelSubmitter, SubmitParams, SubmitResult};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

fn params() -> SubmitParams {
    SubmitParams {
        tracking_number: "1Z999AA10123456784".to_owned(),
        carrier_code: "ups".to_owned(),
        description: "📦 Camera".to_owned(),
    }
}

async fn api(status: u16, mode: SideEffectMode) -> (ParcelApi, wiremock::MockServer) {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("POST"))
        .and(path("/external/add-delivery/"))
        .and(header("api-key", "parcel-key"))
        .and(body_json(json!({
            "tracking_number": "1Z999AA10123456784",
            "carrier_code": "ups",
            "description": "📦 Camera",
            "send_push_confirmation": true,
        })))
        .respond_with(ResponseTemplate::new(status).set_body_string("{\"error\":\"bad carrier\"}"))
        .mount(&server)
        .await;
    let http = omni_testkit::mock_http(&server, &["https://api.parcel.app"]);
    (
        ParcelApi::new(
            http,
            "parcel-key".to_owned(),
            mode,
            TestClock::new(1_800_000_000_000),
        )
        .unwrap(),
        server,
    )
}

#[tokio::test]
async fn posts_the_delivery_and_reports_success() {
    let (api, server) = api(200, SideEffectMode::Live).await;
    assert_eq!(api.submit(&params(), None).await, SubmitResult::Success);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn reports_4xx_as_rejections_and_logs_them() {
    let (api, _server) = api(422, SideEffectMode::Live).await;
    let dir = tempfile::tempdir().unwrap();
    let log = LogFile::make(
        dir.path().join("parcel-tracker/rejections.md"),
        LogFileMode::Append,
    )
    .await
    .unwrap();
    assert_eq!(
        api.submit(&params(), Some(&log)).await,
        SubmitResult::Rejected { status: 422 }
    );
    let written = std::fs::read_to_string(log.path()).unwrap();
    assert!(written.starts_with("## Rejected: 1Z999AA10123456784 (422) — 2027-01-15T0"));
    assert!(written.contains("\"send_push_confirmation\": true"));
    assert!(written.contains("{\"error\":\"bad carrier\"}"));
}

#[tokio::test]
async fn reports_5xx_as_transient_errors() {
    let (api, _server) = api(503, SideEffectMode::Live).await;
    assert_eq!(api.submit(&params(), None).await, SubmitResult::Error);
}

#[tokio::test]
async fn record_mode_never_sends() {
    let (api, server) = api(200, SideEffectMode::Record).await;
    assert_eq!(api.submit(&params(), None).await, SubmitResult::Success);
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(api.recorded().len(), 1);
}
