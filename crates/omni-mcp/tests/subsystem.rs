//! The WP14 wiring contract of `McpPackage` over a test `AppContext`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{TOKEN, other_tools, send};
use omni_config::Config;
use omni_mcp::McpPackage;
use omni_runtime::BootPhase;
use omni_testkit::TestApp;
use serde_json::json;

fn with_token(app: &TestApp, token: Option<&str>) -> omni_runtime::AppContext {
    let mut env = omni_testkit::test_app_env();
    if let Some(token) = token {
        env.insert("OMNI_MCP_TOKEN".to_owned(), token.to_owned());
    }
    let mut ctx = app.ctx.clone();
    ctx.config = Arc::new(Config::from_env(&env).unwrap());
    ctx
}

#[tokio::test]
async fn serves_every_tool_and_registers_tasks_entities_and_the_email_handler() {
    let app = TestApp::new().await;
    let package = McpPackage::new(&with_token(&app, Some(TOKEN)), None).unwrap();
    let own = package.tools().unwrap();
    assert_eq!(own.len(), 16);
    let handler = package.email_handler().unwrap();
    assert_eq!(handler.name(), "McpEvents");
    let mut all = other_tools(&own);
    all.extend(own);
    let subsystem = package.subsystem(all).unwrap();
    let names: Vec<&str> = subsystem.tasks.iter().map(|t| t.name()).collect();
    assert_eq!(names, ["McpEventDelivery"]);
    assert_eq!(subsystem.entities.len(), 6);
    assert_eq!(subsystem.services.len(), 1);
    assert!(subsystem.mcp_tools.is_empty());
    assert_eq!(subsystem.boot_steps.len(), 1);
    assert_eq!(subsystem.boot_steps[0].phase, BootPhase::Reconcile);

    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "events_status", "arguments": {}}})
                .to_string(),
        ))
        .unwrap();
    let exchange = send(&subsystem.router, request).await;
    let message = exchange.message();
    assert_eq!(message["result"]["structuredContent"]["enabled"], true);
    let initialize = send(
        &subsystem.router,
        Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(
                json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(
        initialize.message()["result"]["capabilities"],
        json!({"events": {}, "tools": {"listChanged": true}})
    );

    let mut boot = subsystem.boot_steps.into_iter();
    let step = boot.next().unwrap();
    (step.run)(app.ctx.clone()).await.unwrap();
}

#[tokio::test]
async fn is_unavailable_without_a_token_and_rejects_an_incomplete_tool_set() {
    let app = TestApp::new().await;
    let package = McpPackage::new(&with_token(&app, None), None).unwrap();
    assert!(package.events().is_none());
    assert!(package.email_handler().is_none());
    let own = package.tools().unwrap();
    let mut all = other_tools(&own);
    all.extend(own);
    let subsystem = package.subsystem(all).unwrap();
    assert!(subsystem.tasks.is_empty());
    let exchange = send(
        &subsystem.router,
        Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(exchange.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        exchange.message(),
        json!({"error": "MCP is not configured"})
    );

    let partial = McpPackage::new(&with_token(&app, Some(TOKEN)), None).unwrap();
    let own = partial.tools().unwrap();
    let error = partial.subsystem(own).err().unwrap();
    assert!(error.to_string().contains("missing"));
}
