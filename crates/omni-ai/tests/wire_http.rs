//! End-to-end provider calls against a local mock server: endpoints, auth headers,
//! retries and error mapping (no network).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use omni_ai::costs::CostRecorder;
use omni_ai::{Ai, AiError, CostTag, GenerateRequest, ModelId, ModelRole, Provider};
use omni_core::clock::SharedClock;
use omni_testkit::{TEST_EPOCH_MS, TestStore, mock_http, mock_server, test_clock};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

async fn ai(server: &wiremock::MockServer, store: &TestStore, clock: SharedClock) -> Ai {
    let http = mock_http(
        server,
        &[
            "https://api.openai.com",
            "https://api.anthropic.com",
            "https://generativelanguage.googleapis.com",
        ],
    );
    let keys = BTreeMap::from([
        (Provider::OpenAi, "sk-openai".to_owned()),
        (Provider::Anthropic, "sk-ant".to_owned()),
        (Provider::Google, "g-key".to_owned()),
    ]);
    Ai::with_keys(http, keys, CostRecorder::new(store.store.clone(), clock))
}

// Real time: paused time would auto-advance to the 5-minute call timeout while the
// mock server I/O is pending. The single retry waits the real 2 s backoff.
#[tokio::test]
async fn openai_calls_retry_rate_limits_then_succeed() {
    let clock: SharedClock = test_clock(TEST_EPOCH_MS);
    let store = TestStore::new(clock.clone()).await;
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(429).set_body_json(json!({"error": {"message": "slow down"}})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer sk-openai"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [{"type": "message", "id": "msg_1", "content": [{"type": "output_text", "text": "hi"}]}],
            "usage": {"input_tokens": 5, "output_tokens": 1}
        })))
        .mount(&server)
        .await;
    let ai = ai(&server, &store, clock).await;
    let model = ai
        .model(&ModelId::parse("openai:gpt-6-luna").unwrap())
        .unwrap();
    let (text, usage) = ai
        .generate_text(
            model.as_ref(),
            GenerateRequest::prompt("hello"),
            CostTag::for_role(ModelRole::Triage),
        )
        .await
        .unwrap();
    assert_eq!(text, "hi");
    assert_eq!(usage.input_tokens, 5);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn anthropic_and_gemini_use_their_auth_headers_and_paths() {
    let clock: SharedClock = test_clock(TEST_EPOCH_MS);
    let store = TestStore::new(clock.clone()).await;
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "claude"}], "stop_reason": "end_turn",
            "usage": {"input_tokens": 3, "output_tokens": 2}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-3.5-flash:generateContent"))
        .and(header("x-goog-api-key", "g-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [{"content": {"parts": [{"text": "gemini"}]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 4, "candidatesTokenCount": 1}
        })))
        .mount(&server)
        .await;
    let ai = ai(&server, &store, clock).await;
    for (id, expected) in [
        ("anthropic:claude-sonnet-5-5", "claude"),
        ("google:gemini-3.5-flash", "gemini"),
    ] {
        let model = ai.model(&ModelId::parse(id).unwrap()).unwrap();
        let (text, _) = ai
            .generate_text(
                model.as_ref(),
                GenerateRequest::prompt("hello"),
                CostTag::for_role(ModelRole::Extraction),
            )
            .await
            .unwrap();
        assert_eq!(text, expected);
    }
}

#[tokio::test]
async fn client_errors_are_not_retried_and_carry_the_message() {
    let clock: SharedClock = test_clock(TEST_EPOCH_MS);
    let store = TestStore::new(clock.clone()).await;
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": {"message": "Invalid schema for response_format"}})),
        )
        .mount(&server)
        .await;
    let ai = ai(&server, &store, clock).await;
    let model = ai
        .model(&ModelId::parse("openai:gpt-6-sol").unwrap())
        .unwrap();
    let result = ai
        .generate_text(
            model.as_ref(),
            GenerateRequest::prompt("hello"),
            CostTag::for_role(ModelRole::RecsSelection),
        )
        .await;
    assert!(matches!(
        result,
        Err(AiError::Provider { status: 400, message }) if message == "Invalid schema for response_format"
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[test]
fn missing_keys_are_reported() {
    let clock: SharedClock = test_clock(TEST_EPOCH_MS);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = TestStore::new(clock.clone()).await;
        let ai = Ai::with_keys(
            omni_testkit::no_network(),
            BTreeMap::new(),
            CostRecorder::new(store.store.clone(), clock),
        );
        assert!(matches!(
            ai.model(&ModelId::parse("openai:gpt-6-luna").unwrap()),
            Err(AiError::MissingKey(Provider::OpenAi))
        ));
    });
}
