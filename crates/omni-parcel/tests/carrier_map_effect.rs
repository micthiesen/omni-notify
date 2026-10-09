//! Port of `src/parcel-tracker/carriers/carrierMap.effect.spec.ts`. The TS
//! spec injects a fake streaming request; here a local wiremock server serves
//! the carrier list through the bounded public client.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use omni_core::clock::TestClock;
use omni_http::Url;
use omni_http::public::PublicHttpClient;
use omni_parcel::carriers::carrier_map::CarrierDirectory;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LIST_PATH: &str = "/external/supported_carriers.json";

fn directory(server: &MockServer, max_bytes: usize) -> CarrierDirectory {
    let http = omni_testkit::mock_http(server, &["https://api.parcel.app"]);
    CarrierDirectory::with_limits(
        PublicHttpClient::new(&http).allow_loopback_for_tests(),
        TestClock::new(1_800_000_000_000),
        Url::parse("https://api.parcel.app/external/supported_carriers.json").unwrap(),
        max_bytes,
    )
}

#[tokio::test]
async fn rejects_fixed_length_overflow_before_buffering() {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .and(path(LIST_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("123456789"))
        .mount(&server)
        .await;
    assert_eq!(directory(&server, 8).prompt_codes().await, "");
}

#[tokio::test]
async fn cancels_chunked_overflow() {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .and(path(LIST_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("1234567890"))
        .mount(&server)
        .await;
    assert_eq!(directory(&server, 8).prompt_codes().await, "");
    assert!(directory(&server, 8).valid_codes().await.is_none());
}

#[tokio::test]
async fn coalesces_concurrent_first_successful_refreshes() {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .and(path(LIST_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "canadapost": "Canada Post",
            "ups": {"name": "UPS"},
        })))
        .expect(1)
        .mount(&server)
        .await;
    let directory = Arc::new(directory(&server, 1024));
    let (prompt, codes, _again) = tokio::join!(
        directory.prompt_codes(),
        directory.valid_codes(),
        directory.prompt_codes()
    );
    assert!(prompt.contains("canadapost: Canada Post"));
    assert!(codes.unwrap().contains("ups"));
    server.verify().await;
}

#[tokio::test]
async fn keeps_serving_the_stale_list_when_a_refresh_fails() {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .and(path(LIST_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ups": "UPS"})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let http = omni_testkit::mock_http(&server, &["https://api.parcel.app"]);
    let clock = TestClock::new(1_800_000_000_000);
    let directory = CarrierDirectory::with_limits(
        PublicHttpClient::new(&http).allow_loopback_for_tests(),
        clock.clone(),
        Url::parse("https://api.parcel.app/external/supported_carriers.json").unwrap(),
        1024,
    );
    assert!(directory.valid_codes().await.unwrap().contains("ups"));
    clock.set(1_800_000_000_000 + 25 * 60 * 60 * 1000);
    assert!(directory.valid_codes().await.unwrap().contains("ups"));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn filters_blacklisted_and_nameless_carriers_and_matches_names_on_word_boundaries() {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .and(path(LIST_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "amzl_us": "Amazon Logistics",
            "doordash": "DoorDash",
            "ups": "UPS",
            "nameless": {},
            "dhl": {"name": "DHL Express"},
        })))
        .mount(&server)
        .await;
    let directory = directory(&server, 4096);
    assert_eq!(directory.prompt_codes().await, "ups: UPS\ndhl: DHL Express");
    let patterns = directory.name_patterns().await;
    assert!(patterns.iter().any(|p| p.is_match("Your ups package")));
    assert!(!patterns.iter().any(|p| p.is_match("groups")));
}
