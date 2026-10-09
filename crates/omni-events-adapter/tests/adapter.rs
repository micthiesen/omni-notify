//! The events adapter end to end against a fake Executor.
//!
//! - `the_mcp_2_client_negotiates_the_modern_protocol_against_the_adapter`: there
//!   is no Rust MCP 2 client, so the test performs the pinned client's
//!   negotiation by hand (`server/discover` with the pinned version in header
//!   and metadata) and asserts what the client requires to settle on the
//!   modern era.
//! - The continuation cases drive `Continuations` with a scripted legacy
//!   connector.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use omni_events_adapter::AdapterBuilder;
use omni_events_adapter::auth::{authenticate_oauth, owner_id};
use omni_events_adapter::continuations::{CallRequest, ContinuationError, Continuations};
use omni_events_adapter::legacy::{ElicitationSupport, LegacyError};
use serde_json::{Value, json};
use support::*;

struct Harness {
    adapter: Spawned,
    _executor: Spawned,
    _omni: Spawned,
    inbox: Inbox,
    options: omni_events_adapter::AdapterOptions,
}

async fn harness() -> Harness {
    let executor = spawn(fake_executor()).await;
    let inbox = Inbox::default();
    let omni = spawn(fake_omni(inbox.clone())).await;
    let options = options(&executor.url, &omni.url, USER_ID);
    let built = AdapterBuilder::new(options.clone()).build().unwrap();
    let adapter = spawn(built.router()).await;
    Harness {
        adapter,
        _executor: executor,
        _omni: omni,
        inbox,
        options,
    }
}

async fn request(h: &Harness, method: &str, params: Value, auth: &str) -> reqwest::Response {
    modern(
        &h.adapter.url,
        method,
        params,
        auth,
        "/mcp?elicitation_mode=native",
    )
    .await
}

fn call(owner: &str, auth: &str, params: Value, support: ElicitationSupport) -> CallRequest {
    CallRequest {
        owner: owner.into(),
        authorization: auth.into(),
        mode: Some("native".into()),
        params: object(params),
        support,
    }
}

const FORM: ElicitationSupport = ElicitationSupport {
    form: true,
    url: false,
};
const URL_ONLY: ElicitationSupport = ElicitationSupport {
    form: false,
    url: true,
};

#[tokio::test]
async fn oauth_projection_uses_stable_user_client_identity_and_never_exposes_session_secrets() {
    let h = harness().await;
    let http = reqwest::Client::new();
    let now = jiff::Timestamp::now();
    let bearer = format!("Bearer {TOKEN}");
    let identity = authenticate_oauth(&http, Some(&bearer), &h.options, now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(identity.owner, owner_id(USER_ID, CLIENT_ID));
    assert_eq!(identity.authorization, bearer);
    assert!(identity.expires_at > now);
    assert!(!format!("{identity:?}").contains("must-not-leak"));
    let invalid = authenticate_oauth(&http, Some("Bearer invalid"), &h.options, now).await;
    assert_eq!(invalid, Ok(None));
}

#[tokio::test]
async fn modern_discovery_and_event_forwarding_use_the_same_oauth_owner() {
    let h = harness().await;
    let discovered: Value = request(&h, "server/discover", json!({}), TOKEN)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(discovered["result"]["supportedVersions"], json!([VERSION]));
    let mut capabilities: Vec<_> = discovered["result"]["capabilities"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    capabilities.sort();
    assert_eq!(capabilities, ["events", "resources", "tools"]);
    let events: Value = request(&h, "events/list", json!({}), TOKEN)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(events["result"]["events"], json!([]));
    let received = h.inbox.lock().unwrap().clone().unwrap();
    let header = |name: &str| {
        received
            .headers
            .get(name)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(received.path, "/mcp");
    assert_eq!(header("x-omni-events-owner"), owner_id(USER_ID, CLIENT_ID));
    assert_eq!(
        header("x-omni-events-authorization"),
        format!("Bearer {TOKEN}")
    );
    assert_eq!(header("authorization"), "Bearer omni-test-token");
    assert_eq!(header("mcp-method"), "events/list");
}

#[tokio::test]
async fn the_mcp_2_client_negotiates_the_modern_protocol_against_the_adapter() {
    let h = harness().await;
    let params = json!({
        "_meta": {
            "io.modelcontextprotocol/clientInfo": {"name": "adapter-test", "version": "1.0.0"},
            "io.modelcontextprotocol/clientCapabilities": {},
        }
    });
    let response = request(&h, "server/discover", params, TOKEN).await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let result = &body["result"];
    assert_eq!(body["id"], 7);
    assert_eq!(result["resultType"], "complete");
    assert!(
        result["supportedVersions"]
            .as_array()
            .unwrap()
            .contains(&json!(VERSION))
    );
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"],
        json!({"name": "Executor with Omni Events", "version": "0.1.0"})
    );
}

#[tokio::test]
async fn modern_event_methods_reject_invalid_oauth_before_reaching_omni() {
    let h = harness().await;
    let response = request(&h, "events/list", json!({}), "invalid").await;
    assert_eq!(response.status(), 401);
    assert_eq!(
        response.headers()["www-authenticate"],
        r#"Bearer resource_metadata="https://mcp.syas.ca/.well-known/oauth-protected-resource""#
    );
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        response.headers()["access-control-expose-headers"],
        "WWW-Authenticate"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32001);
    assert!(h.inbox.lock().unwrap().is_none());
}

#[tokio::test]
async fn modern_protocol_header_and_request_metadata_must_agree() {
    let h = harness().await;
    let response = reqwest::Client::new()
        .post(format!("{}/mcp", h.adapter.url))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .header("mcp-protocol-version", VERSION)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "events/list",
                "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2025-06-18"}},
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020);
    assert!(h.inbox.lock().unwrap().is_none());
}

#[tokio::test]
async fn unsupported_modern_version_fails_without_forwarding_to_legacy_executor() {
    let h = harness().await;
    let response = reqwest::Client::new()
        .post(format!("{}/mcp", h.adapter.url))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .header("mcp-protocol-version", "2026-09-01")
        .body(json!({"jsonrpc": "2.0", "id": 10, "method": "tools/list"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32022);
    assert_eq!(
        body["error"]["data"],
        json!({"requested": "2026-09-01", "supported": [VERSION]})
    );
}

#[tokio::test]
async fn legacy_mcp_retains_its_native_query_and_upstream_route() {
    let h = harness().await;
    let response = reqwest::Client::new()
        .post(format!("{}/mcp?elicitation_mode=native", h.adapter.url))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}).to_string())
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["legacy"], true);
    assert_eq!(body["path"], "/mcp?elicitation_mode=native");
}

fn confirm_behavior(params: Value) -> ToolBehavior {
    Arc::new(move |_call, elicit| {
        let params = object(params.clone());
        Box::pin(async move {
            let handler = elicit.expect("elicitation handler installed");
            let answer = handler(params).await;
            Ok(text_result(action_text(&answer)))
        })
    })
}

#[tokio::test]
async fn native_elicitation_resumes_one_invocation_and_rejects_replay_and_another_owner() {
    let fake = FakeConnector::new(confirm_behavior(json!({
        "message": "Confirm?",
        "requestedSchema": {"type": "object", "properties": {}},
    })));
    let manager = Continuations::new(fake.clone(), Duration::from_secs(5), 32);
    let params = json!({"name": "execute", "arguments": {"code": "check()"}});
    let first = manager
        .call(call("owner-1", "Bearer x", params.clone(), FORM))
        .await
        .unwrap();
    assert_eq!(first["resultType"], "input_required");
    assert_eq!(
        first["inputRequests"]["elicitation"]["method"],
        "elicitation/create"
    );
    let mut replay = object(params);
    replay.insert("requestState".into(), first["requestState"].clone());
    replay.insert(
        "inputResponses".into(),
        json!({"elicitation": {"action": "accept", "content": {}}}),
    );
    let replay = Value::Object(replay);
    let foreign = manager
        .call(call("owner-2", "Bearer y", replay.clone(), FORM))
        .await;
    assert_eq!(foreign, Err(ContinuationError::Invalid));
    let result = manager
        .call(call("owner-1", "Bearer x", replay.clone(), FORM))
        .await
        .unwrap();
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["content"][0]["text"], "accept");
    let again = manager
        .call(call("owner-1", "Bearer x", replay, FORM))
        .await;
    assert_eq!(again, Err(ContinuationError::Invalid));
    assert_eq!(fake.calls(), 1);
    assert_eq!(fake.closed(), 1);
}

#[tokio::test]
async fn concurrent_native_replies_consume_a_continuation_once_and_decline_safely() {
    let fake = FakeConnector::new(confirm_behavior(json!({
        "mode": "url",
        "message": "Open link?",
        "url": "https://example.test/",
    })));
    let manager = Continuations::new(fake.clone(), Duration::from_secs(5), 1);
    let params = json!({"name": "execute", "arguments": {"code": "link()"}});
    let first = manager
        .call(call("owner", "Bearer x", params.clone(), URL_ONLY))
        .await
        .unwrap();
    let second = manager
        .call(call("owner", "Bearer x", params.clone(), URL_ONLY))
        .await;
    assert_eq!(second, Err(ContinuationError::Capacity));
    let mut retry = object(params);
    retry.insert("requestState".into(), first["requestState"].clone());
    retry.insert(
        "inputResponses".into(),
        json!({"elicitation": {"action": "decline"}}),
    );
    let retry = Value::Object(retry);
    let (a, b) = tokio::join!(
        manager.call(call("owner", "Bearer x", retry.clone(), FORM)),
        manager.call(call("owner", "Bearer x", retry, FORM)),
    );
    let fulfilled: Vec<_> = [a, b].into_iter().filter_map(Result::ok).collect();
    assert_eq!(fulfilled.len(), 1);
    assert_eq!(fulfilled[0]["content"][0]["text"], "decline");
    assert_eq!(fake.calls(), 1);
}

#[tokio::test]
async fn a_stalled_tool_before_elicitation_expires_and_releases_its_capacity() {
    let fake = FakeConnector::new(Arc::new(|call, _elicit| {
        Box::pin(async move {
            if call == 1 {
                std::future::pending::<()>().await;
            }
            Ok(text_result("recovered"))
        })
    }));
    let manager = Continuations::new(fake.clone(), Duration::from_millis(80), 1);
    let params = json!({"name": "execute", "arguments": {"code": "once()"}});
    let expired = manager
        .call(call("owner", "Bearer x", params.clone(), FORM))
        .await;
    assert_eq!(expired, Err(ContinuationError::Upstream));
    assert_eq!(fake.closed(), 1);
    let recovered = manager
        .call(call("owner", "Bearer x", params, FORM))
        .await
        .unwrap();
    assert_eq!(recovered["content"][0]["text"], "recovered");
    assert_eq!(fake.calls(), 2);
    assert_eq!(fake.closed(), 2);
    assert_eq!(manager.active(), 0);
}

#[tokio::test]
async fn a_stalled_tool_after_accepted_elicitation_expires_without_replay() {
    let fake = FakeConnector::new(Arc::new(|call, elicit| {
        Box::pin(async move {
            if call > 1 {
                return Ok(text_result("recovered"));
            }
            if let Some(handler) = elicit {
                let params = object(json!({
                    "message": "Confirm?",
                    "requestedSchema": {"type": "object"},
                }));
                let _ = handler(params).await;
            }
            std::future::pending::<()>().await;
            Err(LegacyError::Closed)
        })
    }));
    let manager = Continuations::new(fake.clone(), Duration::from_millis(100), 1);
    let params = json!({"name": "execute", "arguments": {"code": "once()"}});
    let first = manager
        .call(call("owner", "Bearer x", params.clone(), FORM))
        .await
        .unwrap();
    let mut reply = object(params.clone());
    reply.insert("requestState".into(), first["requestState"].clone());
    reply.insert(
        "inputResponses".into(),
        json!({"elicitation": {"action": "accept", "content": {}}}),
    );
    let reply = Value::Object(reply);
    let expired = manager
        .call(call("owner", "Bearer x", reply.clone(), FORM))
        .await;
    assert_eq!(expired, Err(ContinuationError::Upstream));
    assert_eq!(fake.closed(), 1);
    let replayed = manager.call(call("owner", "Bearer x", reply, FORM)).await;
    assert_eq!(replayed, Err(ContinuationError::Invalid));
    assert_eq!(fake.calls(), 1);
    let recovered = manager
        .call(call("owner", "Bearer x", params, FORM))
        .await
        .unwrap();
    assert_eq!(recovered["content"][0]["text"], "recovered");
    assert_eq!(fake.closed(), 2);
}

// --- Rust-specific and implicit-contract cases ---

#[tokio::test]
async fn health_answers_without_authentication_and_other_paths_are_not_found() {
    let h = harness().await;
    let client = reqwest::Client::new();
    let health = client
        .get(format!("{}/health", h.adapter.url))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.headers()["cache-control"], "no-store");
    assert_eq!(
        health.json::<Value>().await.unwrap(),
        json!({"status": "ok"})
    );
    let missing = client
        .get(format!("{}/mcp/", h.adapter.url))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        missing.json::<Value>().await.unwrap(),
        json!({"error": "Not found"})
    );
}

#[tokio::test]
async fn expired_or_foreign_sessions_are_unauthorized() {
    let expired = axum::Router::new().fallback(|| async {
        let body = json!({
            "userId": USER_ID,
            "clientId": CLIENT_ID,
            "accessTokenExpiresAt": "2020-01-01T00:00:00.000Z",
        });
        ([("content-type", "application/json")], body.to_string())
    });
    let executor = spawn(expired).await;
    let http = reqwest::Client::new();
    let now = jiff::Timestamp::now();
    let bearer = format!("Bearer {TOKEN}");
    let opts = options(&executor.url, "http://127.0.0.1:9", USER_ID);
    assert_eq!(
        authenticate_oauth(&http, Some(&bearer), &opts, now).await,
        Ok(None)
    );
    let live = spawn(fake_executor()).await;
    let foreign = options(&live.url, "http://127.0.0.1:9", "someone-else");
    assert_eq!(
        authenticate_oauth(&http, Some(&bearer), &foreign, now).await,
        Ok(None)
    );
    let down = axum::Router::new().fallback(|| async { axum::http::StatusCode::BAD_GATEWAY });
    let down = spawn(down).await;
    let unavailable = options(&down.url, "http://127.0.0.1:9", USER_ID);
    assert!(
        authenticate_oauth(&http, Some(&bearer), &unavailable, now)
            .await
            .is_err()
    );
    let adapter = spawn(AdapterBuilder::new(unavailable).build().unwrap().router()).await;
    let response = modern(&adapter.url, "events/list", json!({}), TOKEN, "/mcp").await;
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        -32603
    );
}

#[tokio::test]
async fn unknown_event_methods_and_oversized_modern_requests_are_rejected() {
    let h = harness().await;
    let unknown: Value = request(&h, "events/poll", json!({}), TOKEN)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(unknown["error"]["code"], -32601);
    let unknown: Value = request(&h, "sampling/x", json!({}), TOKEN)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(unknown["error"]["code"], -32601);
    assert!(h.inbox.lock().unwrap().is_none());
    let big = request(
        &h,
        "events/list",
        json!({"pad": "x".repeat(300_000)}),
        TOKEN,
    )
    .await;
    assert_eq!(big.status(), 413);
    assert_eq!(big.json::<Value>().await.unwrap()["error"]["code"], -32600);
    let get = reqwest::Client::new()
        .get(format!("{}/mcp", h.adapter.url))
        .header("mcp-protocol-version", VERSION)
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 400);
    assert_eq!(get.json::<Value>().await.unwrap()["error"]["code"], -32600);
}

#[tokio::test]
async fn legacy_passthrough_keeps_method_query_and_sets_the_project_user_agent() {
    let h = harness().await;
    let response = reqwest::Client::new()
        .get(format!("{}/mcp?x=1", h.adapter.url))
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["method"], "GET");
    assert_eq!(body["path"], "/mcp?x=1");
    assert_eq!(body["userAgent"], omni_events_adapter::USER_AGENT);
}

#[tokio::test]
async fn tools_call_without_advertised_elicitation_still_runs() {
    // A client that does not advertise elicitation still gets its call run.
    let fake = FakeConnector::new(Arc::new(|_call, elicit| {
        Box::pin(async move {
            assert!(elicit.is_none());
            Ok(text_result("ran"))
        })
    }));
    let executor = spawn(fake_executor()).await;
    let built = AdapterBuilder::new(options(&executor.url, "http://127.0.0.1:9", USER_ID))
        .connector(fake.clone())
        .build()
        .unwrap();
    let adapter = spawn(built.router()).await;
    let response = modern(
        &adapter.url,
        "tools/call",
        json!({"name": "execute", "arguments": {}}),
        TOKEN,
        "/mcp",
    )
    .await;
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(body["result"]["content"][0]["text"], "ran");
    assert_eq!(
        body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "Executor with Omni Events"
    );
    assert_eq!(fake.closed(), 1);
}

#[tokio::test]
async fn invalid_tool_calls_are_rejected_before_connecting() {
    let fake = FakeConnector::new(Arc::new(|_call, _elicit| {
        Box::pin(async { Ok(text_result("never")) })
    }));
    let manager = Continuations::new(fake.clone(), Duration::from_secs(5), 1);
    for params in [
        json!({"name": ""}),
        json!({"arguments": {}}),
        json!({"name": "x", "arguments": null}),
        json!({"name": "x", "arguments": [1]}),
        json!({"name": "x", "inputResponses": {}}),
        json!({"name": "x", "requestState": "unknown", "inputResponses": {}}),
    ] {
        let outcome = manager.call(call("owner", "Bearer x", params, FORM)).await;
        assert_eq!(outcome, Err(ContinuationError::Invalid));
    }
    assert_eq!(fake.calls(), 0);
    assert_eq!(manager.active(), 0);
}

#[tokio::test]
async fn tool_results_follow_the_sdk_client_result_rules() {
    let fake = FakeConnector::new(Arc::new(|call, _elicit| {
        Box::pin(async move {
            Ok(match call {
                1 => json!({"structuredContent": {"a": 1}}),
                _ => json!("not an object"),
            })
        })
    }));
    let manager = Continuations::new(fake.clone(), Duration::from_secs(5), 1);
    let params = json!({"name": "execute"});
    let defaulted = manager
        .call(call("owner", "Bearer x", params.clone(), FORM))
        .await
        .unwrap();
    assert_eq!(
        Value::Object(defaulted),
        json!({"resultType": "complete", "structuredContent": {"a": 1}, "content": []})
    );
    let rejected = manager.call(call("owner", "Bearer x", params, FORM)).await;
    assert_eq!(rejected, Err(ContinuationError::Upstream));
    assert_eq!(fake.closed(), 2);
    assert_eq!(manager.active(), 0);
}
