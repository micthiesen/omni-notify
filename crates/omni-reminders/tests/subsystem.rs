//! The assembled subsystem (its wiring contract) and the production HTTP transport.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::type_complexity)]

use std::sync::Arc;

use omni_http::{Method, Url};
use omni_reminders::apple::{AppleRequest, AppleTransport, HttpAppleTransport, TransportError};
use omni_testkit::TestApp;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn assembles_a_disabled_subsystem_without_failing_boot() {
    let app = TestApp::new().await;
    let subsystem = omni_reminders::subsystem(&app.ctx).unwrap();
    assert_eq!(subsystem.name, "reminders");
    assert_eq!(subsystem.tasks.len(), 1);
    assert_eq!(subsystem.tasks[0].name(), "RemindersSession");
    assert_eq!(subsystem.tasks[0].schedule().as_str(), "*/15 * * * *");
    assert_eq!(subsystem.mcp_tools.len(), 15);
    assert!(
        subsystem.entities.is_empty(),
        "Reminders state lives outside the docstore"
    );
    assert_eq!(subsystem.services.len(), 1);
    // Without a public origin the status reports configuration-disabled.
    let router = app.router(&subsystem);
    let (status, body) = app.get_json(&router, "/api/reminders/status").await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        serde_json::json!({"status": {"enabled": false, "phase": "disabled", "reason": "configuration"}})
    );
    let (status, _) = app
        .post_json(&router, "/api/reminders/auth/start", &serde_json::json!({}))
        .await;
    assert_eq!(status, 503);
    // The scheduled check is a no-op while disabled (no storage, no network).
    let service = omni_reminders::build_service(&app.ctx);
    service.health_check().await;
    assert!(!app.ctx.paths.reminders_private.exists());
}

#[tokio::test]
async fn real_transport_keeps_apple_headers_returns_redirects_and_cookies() {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("POST"))
        .and(path("/setup/ws/1/validate"))
        .and(header("user-agent", "python-requests/2.31.0"))
        .and(header("referer", "https://www.icloud.com/"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "a=1; Path=/")
                .append_header("set-cookie", "b=2; Path=/")
                .set_body_string("{}"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(302).append_header("location", "https://attacker.example/"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/large"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; 3 * 1024 * 1024]))
        .mount(&server)
        .await;
    let http = omni_testkit::mock_http(&server, &["https://setup.icloud.com"]);
    let transport: Arc<dyn AppleTransport> = Arc::new(HttpAppleTransport::new(http));
    let request = |p: &str, method: Method| AppleRequest {
        method,
        url: Url::parse(&format!("https://setup.icloud.com{p}")).unwrap(),
        headers: vec![
            ("User-Agent".into(), "python-requests/2.31.0".into()),
            ("Referer".into(), "https://www.icloud.com/".into()),
        ],
        body: Some("null".into()),
    };
    let ok = transport
        .send(request("/setup/ws/1/validate", Method::POST))
        .await
        .unwrap();
    assert_eq!(ok.status, 200);
    assert_eq!(
        ok.set_cookies().collect::<Vec<_>>(),
        ["a=1; Path=/", "b=2; Path=/"]
    );
    let redirect = transport
        .send(request("/redirect", Method::GET))
        .await
        .unwrap();
    assert_eq!(redirect.status, 302);
    assert_eq!(
        transport
            .send(request("/large", Method::GET))
            .await
            .unwrap_err(),
        TransportError::TooLarge
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
