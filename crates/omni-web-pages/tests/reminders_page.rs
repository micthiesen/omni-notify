//! The Reminders page.
//!
//! The page's visible parts come from `reminders_view` (the
//! component renders exactly those parts plus the constant intro), and
//! requests go through the `RemindersTransport` seam with a recording fake.

use std::cell::RefCell;

use futures::executor::block_on;
use omni_api::reminders::{Phase, PublicStatus};
use omni_web_pages::reminders::{
    HTTPS_REQUIRED, INTRO, LOADING, Operation, RemindersHttpRequest, RemindersHttpResponse,
    RemindersTransport, TransportFailure, build_request, reminders_request, reminders_view,
};
use web_sys::{RequestCache, RequestCredentials, RequestRedirect};

/// Answers every request with one canned response and records the requests.
struct FakeTransport {
    response: Result<RemindersHttpResponse, TransportFailure>,
    calls: RefCell<Vec<RemindersHttpRequest>>,
}

impl FakeTransport {
    fn new(status: u16, body: &str) -> Self {
        Self {
            response: Ok(RemindersHttpResponse {
                status,
                body: Some(body.to_owned()),
            }),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl RemindersTransport for FakeTransport {
    async fn send(
        &self,
        request: RemindersHttpRequest,
    ) -> Result<RemindersHttpResponse, TransportFailure> {
        self.calls.borrow_mut().push(request);
        self.response.clone()
    }
}

#[test]
fn keeps_controls_behind_https() {
    let insecure = reminders_view(false, None);
    assert!(
        !insecure.secure,
        "renders {HTTPS_REQUIRED:?} instead of controls"
    );
    assert!(HTTPS_REQUIRED.contains("Open this page over HTTPS"));
    assert!(!insecure.show_start && !insecure.show_verify && !insecure.show_code_form);

    // Even a status that would offer sign-in shows nothing over HTTP.
    let needs_auth = PublicStatus::new(true, Phase::AuthenticationNeeded);
    let insecure_with_status = reminders_view(false, Some(&needs_auth));
    assert!(!insecure_with_status.show_start);

    let secure = reminders_view(true, None);
    assert!(secure.secure);
    assert_eq!(secure.status_line, LOADING);
    assert!(secure.status_line.contains("Loading connection status"));
    assert!(!secure.show_start && !secure.show_code_form && !secure.show_verify);
    assert!(INTRO.contains("Keep Advanced Data Protection enabled"));
}

#[test]
fn offers_sign_in_code_entry_and_access_checks_by_phase() {
    let needs_auth = PublicStatus::new(true, Phase::AuthenticationNeeded);
    let view = reminders_view(true, Some(&needs_auth));
    assert!(view.show_start && view.show_verify && !view.show_code_form);
    assert_eq!(view.status_line, "Sign in to connect iCloud Reminders.");

    let challenged = PublicStatus {
        challenge_id: Some("c1".into()),
        ..needs_auth.clone()
    };
    let view = reminders_view(true, Some(&challenged));
    assert!(view.show_code_form && !view.show_start && view.show_verify);

    let disabled = PublicStatus::new(false, Phase::Disabled);
    let view = reminders_view(true, Some(&disabled));
    assert!(!view.show_start && !view.show_verify && !view.show_code_form);
}

#[test]
fn requests_only_the_fixed_same_origin_route_without_credentials() {
    let transport = FakeTransport::new(
        200,
        r#"{"status":{"enabled":true,"phase":"authentication-needed"}}"#,
    );
    let result = block_on(reminders_request(&transport, Operation::Status, None));
    assert_eq!(result.map(|s| s.phase), Ok(Phase::AuthenticationNeeded));
    let calls = transport.calls.borrow();
    let init = &calls[0];
    assert_eq!(init.path, "/api/reminders/status");
    assert_eq!(init.method, "GET");
    assert_eq!(init.credentials, RequestCredentials::Omit);
    assert_eq!(init.cache, RequestCache::NoStore);
    assert_eq!(init.redirect, RequestRedirect::Error);
    assert!(
        init.headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
    );
    assert_eq!(init.body, None);

    let start = build_request(Operation::Start, None);
    assert_eq!(start.method, "POST");
    assert_eq!(start.path, "/api/reminders/auth/start");
    assert_eq!(start.headers, vec![("Content-Type", "application/json")]);
    assert_eq!(start.body.as_deref(), Some("{}"));
    assert_eq!(start.credentials, RequestCredentials::Omit);
}

#[test]
fn rejects_unexpected_status_payloads() {
    let transport =
        FakeTransport::new(200, r#"{"status":{"enabled":true,"phase":"admin-secret"}}"#);
    let result = block_on(reminders_request(&transport, Operation::Status, None));
    assert_eq!(result, Err("Invalid Reminders response".to_owned()));
}

#[test]
fn shows_bounded_stage_details_instead_of_a_generic_502() {
    for stage in [
        "account-session",
        "second-factor-options",
        "device-notification",
        "code-verification",
        "protected-data-access",
    ] {
        let status = serde_json::json!({
            "enabled": true,
            "phase": "unsupported-protocol",
            "diagnostic": { "stage": stage, "category": "apple-response", "httpStatus": 421 },
        });
        let body = serde_json::json!({ "status": status }).to_string();
        let transport = FakeTransport::new(502, &body);
        let result = block_on(reminders_request(&transport, Operation::Start, None))
            .unwrap_or_else(|e| panic!("{stage}: {e}"));
        assert_eq!(serde_json::to_value(&result).ok(), Some(status), "{stage}");
        let view = reminders_view(true, Some(&result));
        assert_eq!(
            view.diagnostic.as_deref(),
            Some(format!("Failed step: {stage} (apple-response, Apple HTTP 421).").as_str())
        );
    }
}

#[test]
fn keeps_proxy_failures_distinct_and_never_renders_arbitrary_error_bodies() {
    let transport = FakeTransport::new(502, "secret upstream HTML");
    let result = block_on(reminders_request(&transport, Operation::Start, None));
    assert_eq!(result, Err("Reminders request failed (502)".to_owned()));
}

#[test]
fn rate_limits_code_failures_and_unreachable_service_have_fixed_messages() {
    let limited = FakeTransport::new(429, "{}");
    assert_eq!(
        block_on(reminders_request(&limited, Operation::Verify, None)),
        Err("Too many requests. Try again later.".to_owned())
    );
    let code = FakeTransport::new(500, "{}");
    assert_eq!(
        block_on(reminders_request(&code, Operation::Code, None)),
        Err(
            "Code submission was not confirmed. Select Check access before trying again."
                .to_owned()
        )
    );
    let unreachable = FakeTransport {
        response: Err(TransportFailure),
        calls: RefCell::new(Vec::new()),
    };
    assert_eq!(
        block_on(reminders_request(&unreachable, Operation::Status, None)),
        Err("Could not reach the Reminders service".to_owned())
    );
    // An authenticated status on a failed verify is not trusted.
    let authed = FakeTransport::new(
        500,
        r#"{"status":{"enabled":true,"phase":"authenticated"}}"#,
    );
    assert_eq!(
        block_on(reminders_request(&authed, Operation::Verify, None)),
        Err("Reminders request failed (500)".to_owned())
    );
}

#[test]
fn code_submissions_post_the_challenge_and_code() {
    let input = omni_web_pages::reminders::CodeInput {
        challenge_id: "c1".into(),
        code: "123456".into(),
    };
    let request = build_request(Operation::Code, Some(&input));
    assert_eq!(request.path, "/api/reminders/auth/code");
    assert_eq!(
        request.body.as_deref(),
        Some(r#"{"challengeId":"c1","code":"123456"}"#)
    );
    assert!(omni_web_pages::reminders::is_six_digit_code("012345"));
    assert!(!omni_web_pages::reminders::is_six_digit_code("12345"));
    assert!(!omni_web_pages::reminders::is_six_digit_code(
        "１２３４５６"
    ));
}

#[test]
fn badges_the_connection_phase_with_a_shape_and_word() {
    use omni_web_kit::components::StatusKind;
    use omni_web_pages::reminders::phase_badge;

    assert_eq!(phase_badge(None), (StatusKind::Running, "Checking"));
    let connected = PublicStatus::new(true, Phase::Authenticated);
    assert_eq!(phase_badge(Some(&connected)), (StatusKind::Ok, "Connected"));
    let needs_auth = PublicStatus::new(true, Phase::AuthenticationNeeded);
    assert_eq!(
        phase_badge(Some(&needs_auth)),
        (StatusKind::Warn, "Sign-in needed")
    );
    let challenged = PublicStatus {
        challenge_id: Some("c1".into()),
        ..needs_auth.clone()
    };
    assert_eq!(
        phase_badge(Some(&challenged)),
        (StatusKind::Warn, "Code needed")
    );
    let unsupported = PublicStatus::new(true, Phase::UnsupportedProtocol);
    assert_eq!(phase_badge(Some(&unsupported)).0, StatusKind::Fault);

    // Check access leads only when it is the sole action.
    assert!(!reminders_view(true, Some(&needs_auth)).verify_is_primary());
    assert!(!reminders_view(true, Some(&challenged)).verify_is_primary());
    let approval = PublicStatus::new(true, Phase::AwaitingDeviceApproval);
    assert!(reminders_view(true, Some(&approval)).verify_is_primary());
}
