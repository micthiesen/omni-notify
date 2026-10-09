//! The delivery extraction schema, plus the model-backed extractor with
//! scripted responses.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use omni_ai::{FinishReason, GenerateResponse, ModelRole};
use omni_email::activity::LlmCost;
use omni_http::public::PublicHttpClient;
use omni_parcel::carriers::carrier_map::CarrierDirectory;
use omni_parcel::error::ParcelError;
use omni_parcel::extraction::{
    DeliveryExtractor, ExtractionEmail, ModelExtractor, decode_extraction,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

#[test]
fn should_accept_valid_extraction_with_deliveries() {
    let result = decode_extraction(json!({
        "deliveries": [{
            "tracking_number": "1Z999AA10123456784",
            "carrier_candidates": ["ups"],
            "description": "Electronics order",
        }]
    }))
    .unwrap();
    assert_eq!(result.deliveries.len(), 1);
    assert_eq!(result.deliveries[0].tracking_number, "1Z999AA10123456784");
    assert_eq!(result.deliveries[0].carrier_candidates, ["ups"]);
}

#[test]
fn should_accept_multiple_ranked_carrier_candidates() {
    let result = decode_extraction(json!({
        "deliveries": [{
            "tracking_number": "DCM123456789",
            "carrier_candidates": ["dicom", "gls", "canpost"],
            "description": "Kitchen Knife Set",
        }]
    }))
    .unwrap();
    assert_eq!(
        result.deliveries[0].carrier_candidates,
        ["dicom", "gls", "canpost"]
    );
}

#[test]
fn should_accept_empty_deliveries_array() {
    assert!(
        decode_extraction(json!({ "deliveries": [] }))
            .unwrap()
            .deliveries
            .is_empty()
    );
}

#[test]
fn should_accept_multiple_deliveries() {
    let result = decode_extraction(json!({
        "deliveries": [
            {"tracking_number": "1Z999AA10123456784", "carrier_candidates": ["ups"], "description": "Order 1"},
            {"tracking_number": "9400111899223100315842", "carrier_candidates": ["usps", "canpost"], "description": "Order 2"},
        ]
    }))
    .unwrap();
    assert_eq!(result.deliveries.len(), 2);
}

#[test]
fn should_reject_missing_tracking_number() {
    assert!(
        decode_extraction(json!({
            "deliveries": [{"carrier_candidates": ["ups"], "description": "Test"}]
        }))
        .is_err()
    );
}

#[test]
fn should_reject_missing_carrier_candidates() {
    assert!(
        decode_extraction(json!({
            "deliveries": [{"tracking_number": "1Z999AA10123456784", "description": "Test"}]
        }))
        .is_err()
    );
}

#[test]
fn should_reject_empty_carrier_candidates() {
    assert!(decode_extraction(json!({
        "deliveries": [{"tracking_number": "1Z999AA10123456784", "carrier_candidates": [], "description": "Test"}]
    }))
    .is_err());
}

#[test]
fn should_reject_missing_deliveries_key() {
    assert!(decode_extraction(json!({})).is_err());
}

fn shipment() -> ExtractionEmail {
    ExtractionEmail {
        subject: "Order Shipped #123456".to_owned(),
        from: "shop@example.com".to_owned(),
        text_body: "Your tracking number is 1Z999AA10123456784".to_owned(),
        links: vec!["https://shop.test/track/1Z999AA10123456784".to_owned()],
    }
}

async fn extractor(app: &omni_testkit::TestApp) -> (ModelExtractor, wiremock::MockServer) {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .and(path("/external/supported_carriers.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ups": "UPS",
            "canpost": {"name": "Canada Post"},
            "doordash": "DoorDash",
        })))
        .mount(&server)
        .await;
    let http = omni_testkit::mock_http(&server, &["https://api.parcel.app"]);
    let carriers = Arc::new(
        CarrierDirectory::new(
            PublicHttpClient::new(&http).allow_loopback_for_tests(),
            app.ctx.clock.clone(),
        )
        .unwrap(),
    );
    (
        ModelExtractor::new(app.ctx.ai.clone(), app.ctx.config.clone(), carriers),
        server,
    )
}

#[tokio::test]
async fn extracts_ranked_candidates_and_the_call_cost() {
    let app = omni_testkit::TestApp::new().await;
    app.ai.script(
        ModelRole::Extraction,
        vec![GenerateResponse {
            usage: omni_ai::Usage {
                input_tokens: 2_000,
                output_tokens: 100,
                ..omni_ai::Usage::default()
            },
            ..GenerateResponse::text(
                r#"{"deliveries":[{"tracking_number":"1Z999AA10123456784","carrier_candidates":["ups","canpost","dhl","fedex"],"description":"📦 Camera"}]}"#,
            )
        }],
    );
    let (extractor, _server) = extractor(&app).await;
    let result = extractor.extract(&shipment(), None).await.unwrap();
    assert_eq!(result.deliveries.len(), 1);
    assert_eq!(
        result.deliveries[0].carrier_candidates,
        ["ups", "canpost", "dhl"]
    );
    assert!(matches!(result.cost, LlmCost::Cents(cents) if cents > 0.0));

    let requests = app.ai.requests();
    let prompt = match &requests[0].1.messages[0].content[0] {
        omni_ai::ContentPart::Text { text } => text.clone(),
        other => panic!("unexpected content {other:?}"),
    };
    assert!(prompt.contains("ups: UPS\ncanpost: Canada Post"));
    assert!(!prompt.contains("doordash"));
    assert!(prompt.contains("URLs from the email (tracking numbers sometimes appear only inside these):\nhttps://shop.test/track/1Z999AA10123456784"));
    assert!(prompt.contains("From: shop@example.com\nSubject: Order Shipped #123456"));
}

#[tokio::test]
async fn provider_failures_are_transient_and_schema_failures_are_not() {
    let app = omni_testkit::TestApp::new().await;
    app.ai.script_failure(
        ModelRole::Extraction,
        omni_testkit::FakeFailure {
            status: 400,
            message: "model timeout".to_owned(),
        },
    );
    app.ai.script(
        ModelRole::Extraction,
        vec![GenerateResponse {
            finish: FinishReason::Stop,
            ..GenerateResponse::text(r#"{"deliveries":[{"tracking_number":"X","carrier_candidates":[],"description":"d"}]}"#)
        }],
    );
    let (extractor, _server) = extractor(&app).await;
    let first = extractor.extract(&shipment(), None).await.unwrap_err();
    assert!(matches!(
        first,
        ParcelError::Extraction {
            transient: true,
            ..
        }
    ));
    assert!(first.to_string().starts_with("Parcel extraction failed: "));
    let second = extractor.extract(&shipment(), None).await.unwrap_err();
    assert!(matches!(
        second,
        ParcelError::Extraction {
            transient: false,
            ..
        }
    ));
}
