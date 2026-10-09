//! Port of `src/mcp/events/webhook.spec.ts` (all cases kept), plus the
//! `SideEffectMode::Record` contract of the production transport.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::future::BoxFuture;
use hmac::{Hmac, KeyInit as _, Mac as _};
use omni_core::clock::TestClock;
use omni_http::SideEffectMode;
use omni_http::public::PublicHttpClient;
use omni_mcp::events::webhook::{
    LiveWebhookTransport, WebhookClient, WebhookDestination, WebhookError, WebhookEvent,
    WebhookPort, WebhookResponse, WebhookTransport, validate_callback_url, validate_signing_secret,
    webhook_headers,
};
use serde_json::{Value, json};
use sha2::Sha256;

fn secret_of(byte: u8, len: usize) -> String {
    format!("whsec_{}", STANDARD.encode(vec![byte; len]))
}

fn destination() -> WebhookDestination {
    WebhookDestination {
        id: "sub_fixture".to_owned(),
        url: "https://example.com/callback".to_owned(),
        secret: secret_of(7, 32),
        previous_secret: None,
    }
}

type Respond = dyn Fn(&str, &[(&'static str, String)]) -> WebhookResponse + Send + Sync;

struct Transport(Box<Respond>);

impl WebhookTransport for Transport {
    fn post<'a>(
        &'a self,
        _url: &'a str,
        body: String,
        headers: Vec<(&'static str, String)>,
    ) -> BoxFuture<'a, Result<WebhookResponse, WebhookError>> {
        let response = (self.0)(&body, &headers);
        Box::pin(async move { Ok(response) })
    }
}

fn client(
    respond: impl Fn(&str, &[(&'static str, String)]) -> WebhookResponse + Send + Sync + 'static,
) -> WebhookClient {
    WebhookClient::new(
        Arc::new(Transport(Box::new(respond))),
        TestClock::new(1_790_000_000_000),
    )
}

fn header<'a>(headers: &'a [(&'static str, String)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .unwrap()
}

#[test]
fn matches_standard_webhooks_exact_body_bytes_and_rotates_keys() {
    let body = r#"{"hello":"world"}"#;
    let rotating = WebhookDestination {
        previous_secret: Some(secret_of(8, 24)),
        ..destination()
    };
    let headers = webhook_headers(&rotating, "evt_fixture", 123, body).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(&[7u8; 32]).unwrap();
    mac.update(format!("evt_fixture.123.{body}").as_bytes());
    let expected = STANDARD.encode(mac.finalize().into_bytes());
    let signatures: Vec<&str> = header(&headers, "webhook-signature").split(' ').collect();
    assert_eq!(signatures[0], format!("v1,{expected}"));
    assert_eq!(signatures.len(), 2);
    assert_eq!(header(&headers, "webhook-id"), "evt_fixture");
    assert_eq!(header(&headers, "webhook-timestamp"), "123");
    assert_eq!(header(&headers, "X-MCP-Subscription-Id"), "sub_fixture");
    assert_eq!(
        header(&headers, "user-agent"),
        "OpenAI File Downloader, XaiImageApiFetch/1.0"
    );
}

#[test]
fn rejects_invalid_secrets_and_nonpublic_non_https_callback_syntax() {
    for value in [
        "plain".to_owned(),
        "whsec_a".to_owned(),
        secret_of(0, 23),
        secret_of(0, 65),
    ] {
        assert!(validate_signing_secret(&value).is_none(), "{value}");
    }
    assert_eq!(
        validate_signing_secret(&secret_of(7, 32)).unwrap(),
        vec![7u8; 32]
    );
    for value in [
        "http://example.com",
        "https://localhost",
        "https://127.1",
        "https://[::ffff:127.0.0.1]",
        "https://example.com/#secret",
        "https://user:pass@example.com",
        "https://[fec0::1]",
    ] {
        assert!(validate_callback_url(value).is_none(), "{value}");
    }
    assert_eq!(
        validate_callback_url("https://Example.com/callback#")
            .unwrap()
            .as_str(),
        "https://example.com/callback#"
    );
}

#[tokio::test]
async fn verifies_a_fresh_challenge_signs_it_and_never_sends_mail_during_verification() {
    let challenges = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = challenges.clone();
    let client = client(move |body, headers| {
        let value: Value = serde_json::from_str(body).unwrap();
        assert_eq!(value["type"], "verification");
        let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["challenge", "type"]);
        assert!(header(headers, "webhook-id").starts_with("msg_verification_"));
        let challenge = value["challenge"].as_str().unwrap().to_owned();
        seen.lock().unwrap().push(challenge.clone());
        WebhookResponse {
            status: 200,
            body: json!({ "challenge": challenge }).to_string(),
        }
    });
    client.verify(&destination()).await.unwrap();
    client.verify(&destination()).await.unwrap();
    let challenges = challenges.lock().unwrap();
    assert_ne!(challenges[0], challenges[1]);
}

#[tokio::test]
async fn rejects_wrong_echo_redirects_and_unsuccessful_verification() {
    for status in [200, 302, 500] {
        let client = client(move |_, _| WebhookResponse {
            status,
            body: r#"{"challenge":"wrong"}"#.to_owned(),
        });
        assert!(client.verify(&destination()).await.is_err());
    }
}

#[tokio::test]
async fn preserves_the_event_id_and_exact_payload_on_delivery() {
    let event = WebhookEvent {
        event_id: "evt_fixture".to_owned(),
        name: "email.received".to_owned(),
        timestamp: "2026-10-01T00:00:00Z".to_owned(),
        data: json!({"messageId": "fixture"})
            .as_object()
            .cloned()
            .unwrap(),
    };
    let client = client(|body, headers| {
        assert_eq!(
            body,
            r#"{"eventId":"evt_fixture","name":"email.received","timestamp":"2026-10-01T00:00:00Z","data":{"messageId":"fixture"},"cursor":null}"#
        );
        assert_eq!(header(headers, "webhook-id"), "evt_fixture");
        WebhookResponse {
            status: 410,
            body: String::new(),
        }
    });
    assert_eq!(client.deliver(&destination(), &event).await.unwrap(), 410);
}

#[tokio::test]
async fn record_mode_sends_nothing_yet_completes_the_handshake() {
    let transport = LiveWebhookTransport::new(
        PublicHttpClient::new(&omni_testkit::no_network()),
        SideEffectMode::Record,
    );
    let client = WebhookClient::new(Arc::new(transport), TestClock::new(1_790_000_000_000));
    client.verify(&destination()).await.unwrap();
    let event = WebhookEvent {
        event_id: "evt".to_owned(),
        name: "email.received".to_owned(),
        timestamp: "2026-10-01T00:00:00.000Z".to_owned(),
        data: serde_json::Map::new(),
    };
    assert_eq!(client.deliver(&destination(), &event).await.unwrap(), 204);
}

#[tokio::test]
async fn live_mode_refuses_non_public_addresses_before_connecting() {
    let transport = LiveWebhookTransport::new(
        PublicHttpClient::new(&omni_testkit::no_network()),
        SideEffectMode::Live,
    );
    let error = transport
        .post("https://127.0.0.1/callback", "{}".to_owned(), Vec::new())
        .await
        .unwrap_err();
    assert_eq!(error.reason.as_str(), "delivery_failed");
}
