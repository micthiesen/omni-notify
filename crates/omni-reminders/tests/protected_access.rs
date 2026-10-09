//! Port of `src/icloud/protectedAccess.spec.ts`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::type_complexity)]

use std::sync::Mutex;
use std::time::Duration;

use futures::future::BoxFuture;
use omni_reminders::protected_access::{
    PcsEndpoint, PcsError, PcsFailureReason, PcsOperation, PcsRequester, PcsResponse,
    ProtectedAccess, request_protected_access,
};
use serde_json::{Value, json};

type Call = (PcsOperation, PcsEndpoint, Option<Value>);

#[derive(Default)]
struct Fixture {
    responses: Mutex<Vec<Result<PcsResponse, &'static str>>>,
    calls: Mutex<Vec<Call>>,
}

impl Fixture {
    fn new(responses: Vec<Result<PcsResponse, &'static str>>) -> std::sync::Arc<Self> {
        let mut responses = responses;
        responses.reverse();
        std::sync::Arc::new(Self {
            responses: Mutex::new(responses),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl PcsRequester<&'static str> for Fixture {
    fn request(
        &self,
        operation: PcsOperation,
        endpoint: PcsEndpoint,
        body: Option<Value>,
    ) -> BoxFuture<'_, Result<PcsResponse, &'static str>> {
        self.calls.lock().unwrap().push((operation, endpoint, body));
        let next = self.responses.lock().unwrap().pop();
        Box::pin(async move { next.unwrap_or(Err("Unexpected request")) })
    }
}

fn ok(data: Value) -> Result<PcsResponse, &'static str> {
    Ok(PcsResponse { status: 200, data })
}

fn approved() -> Value {
    json!({"isICDRSDisabled": true, "isDeviceConsentedForPCS": true})
}

fn pending() -> Value {
    json!({"status": "pending", "message": "Requested the device to upload cookies."})
}

#[tokio::test]
async fn skips_pcs_when_icdrs_is_enabled() {
    let fixture = Fixture::new(vec![ok(json!({"isICDRSDisabled": false}))]);
    assert_eq!(
        request_protected_access(fixture.as_ref(), "reminders").await,
        Ok(ProtectedAccess::NotRequired)
    );
    assert_eq!(
        fixture.calls(),
        [(
            PcsOperation::State,
            PcsEndpoint::RequestWebAccessState,
            None
        )]
    );
}

#[tokio::test]
async fn requires_device_approval_when_consent_is_false_or_undefined() {
    for state in [
        json!({"isICDRSDisabled": true, "isDeviceConsentedForPCS": false}),
        json!({"isICDRSDisabled": true}),
    ] {
        let fixture = Fixture::new(vec![
            ok(state),
            ok(json!({"isDeviceConsentNotificationSent": true})),
        ]);
        assert_eq!(
            request_protected_access(fixture.as_ref(), "reminders").await,
            Ok(ProtectedAccess::ConsentRequired)
        );
        let calls = fixture.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1],
            (
                PcsOperation::Consent,
                PcsEndpoint::EnableDeviceConsentForPcs,
                None
            )
        );
    }
}

#[tokio::test(start_paused = true)]
async fn polls_known_pending_states_and_marks_only_the_first_request_as_user_initiated() {
    let fixture = Fixture::new(vec![
        ok(approved()),
        ok(pending()),
        ok(json!({"status": "pending", "message": "Cookies not available yet on server."})),
        ok(json!({"status": "success"})),
    ]);
    let started = tokio::time::Instant::now();
    assert_eq!(
        request_protected_access(fixture.as_ref(), "future-items").await,
        Ok(ProtectedAccess::Ready)
    );
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    let body =
        |first: bool| Some(json!({"appName": "future-items", "derivedFromUserAction": first}));
    assert_eq!(
        fixture.calls()[1..],
        [
            (PcsOperation::Cookies, PcsEndpoint::RequestPcs, body(true)),
            (PcsOperation::Cookies, PcsEndpoint::RequestPcs, body(false)),
            (PcsOperation::Cookies, PcsEndpoint::RequestPcs, body(false)),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn stops_after_ten_cookie_attempts() {
    let mut responses = vec![ok(approved())];
    responses.extend((0..10).map(|_| ok(pending())));
    let fixture = Fixture::new(responses);
    let started = tokio::time::Instant::now();
    assert_eq!(
        request_protected_access(fixture.as_ref(), "reminders").await,
        Ok(ProtectedAccess::ConsentRequired)
    );
    assert_eq!(started.elapsed(), Duration::from_secs(45));
    assert_eq!(fixture.calls().len(), 11);
}

#[tokio::test(start_paused = true)]
async fn interruption_cancels_polling() {
    let fixture = Fixture::new(vec![ok(approved()), ok(pending())]);
    let poll = request_protected_access(fixture.as_ref(), "reminders");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), poll)
            .await
            .is_err()
    );
    assert_eq!(fixture.calls().len(), 2);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(fixture.calls().len(), 2);
}

#[tokio::test]
async fn fails_without_retry_or_raw_response_disclosure() {
    let http = |status: u16| {
        Ok(PcsResponse {
            status,
            data: json!("secret"),
        })
    };
    let cases: Vec<(
        Vec<Result<PcsResponse, &'static str>>,
        PcsOperation,
        u16,
        PcsFailureReason,
    )> = vec![
        (
            vec![ok(json!({}))],
            PcsOperation::State,
            200,
            PcsFailureReason::InvalidResponse,
        ),
        (
            vec![http(503)],
            PcsOperation::State,
            503,
            PcsFailureReason::Http,
        ),
        (
            vec![ok(json!({"isICDRSDisabled": "false"}))],
            PcsOperation::State,
            200,
            PcsFailureReason::InvalidResponse,
        ),
        (
            vec![ok(
                json!({"isICDRSDisabled": false, "isDeviceConsentedForPCS": "secret"}),
            )],
            PcsOperation::State,
            200,
            PcsFailureReason::InvalidResponse,
        ),
        (
            vec![ok(Value::Null)],
            PcsOperation::State,
            200,
            PcsFailureReason::InvalidResponse,
        ),
        (
            vec![ok(json!({"isICDRSDisabled": true})), http(403)],
            PcsOperation::Consent,
            403,
            PcsFailureReason::Http,
        ),
        (
            vec![
                ok(json!({"isICDRSDisabled": true})),
                ok(json!({"isDeviceConsentNotificationSent": false})),
            ],
            PcsOperation::Consent,
            200,
            PcsFailureReason::ConsentNotSent,
        ),
        (
            vec![ok(json!({"isICDRSDisabled": true})), ok(json!({}))],
            PcsOperation::Consent,
            200,
            PcsFailureReason::InvalidResponse,
        ),
        (
            vec![ok(approved()), http(429)],
            PcsOperation::Cookies,
            429,
            PcsFailureReason::Http,
        ),
        (
            vec![
                ok(approved()),
                ok(json!({"status": "failure", "message": "secret"})),
            ],
            PcsOperation::Cookies,
            200,
            PcsFailureReason::UnknownState,
        ),
        (
            vec![ok(approved()), ok(json!({"status": "failure"}))],
            PcsOperation::Cookies,
            200,
            PcsFailureReason::UnknownState,
        ),
        (
            vec![
                ok(approved()),
                ok(json!({"status": 1, "message": "secret"})),
            ],
            PcsOperation::Cookies,
            200,
            PcsFailureReason::InvalidResponse,
        ),
    ];
    for (index, (responses, operation, status, reason)) in cases.into_iter().enumerate() {
        let count = responses.len();
        let fixture = Fixture::new(responses);
        let result = request_protected_access(fixture.as_ref(), "reminders").await;
        let Err(PcsError::Pcs(error)) = result else {
            panic!("case {index}: {result:?}");
        };
        assert_eq!(
            (error.operation, error.status, error.reason),
            (operation, Some(status), reason),
            "case {index}"
        );
        assert!(!format!("{error:?} {error}").contains("secret"));
        assert_eq!(fixture.calls().len(), count, "case {index}");
    }
}

#[tokio::test]
async fn preserves_request_failures_without_retry() {
    let fixture = Fixture::new(vec![Err("transport")]);
    assert_eq!(
        request_protected_access(fixture.as_ref(), "reminders").await,
        Err(PcsError::Request("transport"))
    );
    assert_eq!(fixture.calls().len(), 1);
}

#[tokio::test]
async fn rejects_invalid_service_names() {
    let long = "a".repeat(65);
    for app in ["", "Reminders", "a/b", "a b", long.as_str()] {
        let fixture = Fixture::new(vec![]);
        let result = request_protected_access(fixture.as_ref(), app).await;
        assert!(
            matches!(result, Err(PcsError::Pcs(e)) if e.reason == PcsFailureReason::InvalidAppName),
            "{app:?}"
        );
        assert!(fixture.calls().is_empty());
    }
}
