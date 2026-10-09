//! The Apple Reminders client over a scripted [`AppleTransport`] that records
//! each request. The transport returns whole bounded responses, so the client's
//! operation timeout covers headers and body together. "Aborts" checks that the
//! pending transport future is dropped (reqwest cancels on drop).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::type_complexity)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_http::{Method, SideEffectMode};
use omni_reminders::apple::{
    AppleBeginResult, AppleClientOptions, AppleErrorKind, AppleRemindersClient, AppleRequest,
    AppleResponse, AppleSession, AppleTransport, CkPath, SessionStorage, TransportError,
};
use omni_reminders::protected_access::ProtectedAccess;
use serde_json::{Value, json};

type Responder = dyn Fn(&AppleRequest, usize) -> BoxFuture<'static, Result<AppleResponse, TransportError>>
    + Send
    + Sync;

struct Fake {
    responder: Box<Responder>,
    calls: Mutex<Vec<AppleRequest>>,
}

impl AppleTransport for Fake {
    fn send(&self, request: AppleRequest) -> BoxFuture<'_, Result<AppleResponse, TransportError>> {
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(request.clone());
            calls.len()
        };
        (self.responder)(&request, index)
    }
}

#[derive(Default)]
struct Storage {
    initial: Mutex<Value>,
    writes: Mutex<Vec<Value>>,
}

impl SessionStorage for Storage {
    fn load(&self) -> BoxFuture<'_, Result<Value, ()>> {
        let value = self.initial.lock().unwrap().clone();
        Box::pin(async move { Ok(value) })
    }

    fn save(&self, session: Value) -> BoxFuture<'_, Result<(), ()>> {
        self.writes.lock().unwrap().push(session);
        Box::pin(async { Ok(()) })
    }
}

struct Client {
    instance: AppleRemindersClient,
    transport: Arc<Fake>,
    storage: Arc<Storage>,
}

impl Client {
    fn calls(&self) -> Vec<AppleRequest> {
        self.transport.calls.lock().unwrap().clone()
    }

    fn writes(&self) -> Vec<Value> {
        self.storage.writes.lock().unwrap().clone()
    }
}

fn saved() -> Value {
    json!({
        "clientId": "auth-test",
        "sessionToken": "session",
        "trustToken": "trust",
        "accountCountry": "US",
        "ckBaseUrl": "https://ckdatabasews.icloud.com/database/1/com.apple.reminders/production/private",
    })
}

fn client_with(
    responder: impl Fn(
        &AppleRequest,
        usize,
    ) -> BoxFuture<'static, Result<AppleResponse, TransportError>>
    + Send
    + Sync
    + 'static,
    initial: Value,
    timeout: Option<Duration>,
) -> Client {
    let transport = Arc::new(Fake {
        responder: Box::new(responder),
        calls: Mutex::new(Vec::new()),
    });
    let storage = Arc::new(Storage {
        initial: Mutex::new(initial),
        writes: Mutex::new(Vec::new()),
    });
    let instance = AppleRemindersClient::new(AppleClientOptions {
        account: "test@example.com".into(),
        password: "test-password".into(),
        storage: storage.clone(),
        transport: transport.clone(),
        clock: omni_testkit::test_clock(1_800_000_000_000),
        timeout,
        side_effects: SideEffectMode::Live,
    });
    Client {
        instance,
        transport,
        storage,
    }
}

fn client(
    responder: impl Fn(&AppleRequest, usize) -> AppleResponse + Send + Sync + 'static,
    initial: Value,
) -> Client {
    client_with(
        move |request, index| {
            let response = responder(request, index);
            Box::pin(async move { Ok(response) })
        },
        initial,
        None,
    )
}

fn json_response(status: u16, value: Value, headers: &[(&str, &str)]) -> AppleResponse {
    AppleResponse::json(status, &value, headers)
}

fn ok(value: Value) -> AppleResponse {
    json_response(200, value, &[])
}

fn path(request: &AppleRequest) -> String {
    request.url.path().to_owned()
}

/// A future that never resolves and records being dropped.
fn never(dropped: Arc<AtomicBool>) -> BoxFuture<'static, Result<AppleResponse, TransportError>> {
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let guard = Guard(dropped);
    Box::pin(async move {
        let _guard = guard;
        futures::future::pending::<()>().await;
        Err(TransportError::Network)
    })
}

#[tokio::test(start_paused = true)]
async fn lets_a_current_record_query_take_longer_than_the_normal_15_seconds() {
    let x = client_with(
        |_, _| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(20)).await;
                Ok(ok(json!({"records": []})))
            })
        },
        saved(),
        None,
    );
    let result = x.instance.cloudkit(CkPath::RecordsQuery, json!({})).await;
    assert_eq!(result.unwrap(), json!({"records": []}));
    assert_eq!(x.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn bounds_a_stalled_query_without_replay() {
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let x = client_with(move |_, _| never(flag.clone()), saved(), None);
    let started = tokio::time::Instant::now();
    let error = x
        .instance
        .cloudkit(CkPath::RecordsQuery, json!({}))
        .await
        .unwrap_err();
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert_eq!(error.kind, AppleErrorKind::TransientOutage);
    assert_eq!(error.reason, "Apple request timed out");
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(x.calls().len(), 1);
    assert!(x.writes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn preserves_the_timeout_for_lookup_and_overridden_queries() {
    for (path, timeout, expected) in [
        (CkPath::RecordsLookup, None, Duration::from_secs(15)),
        (
            CkPath::RecordsQuery,
            Some(Duration::from_secs(2)),
            Duration::from_secs(2),
        ),
    ] {
        let x = client_with(
            |_, _| never(Arc::new(AtomicBool::new(false))),
            saved(),
            timeout,
        );
        let started = tokio::time::Instant::now();
        let error = x.instance.cloudkit(path, json!({})).await.unwrap_err();
        assert_eq!(started.elapsed(), expected);
        assert_eq!(error.kind, AppleErrorKind::TransientOutage);
        assert_eq!(x.calls().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn aborts_an_interrupted_query_without_replay_or_persistence() {
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let x = client_with(move |_, _| never(flag.clone()), saved(), None);
    let call = x.instance.cloudkit(CkPath::RecordsQuery, json!({}));
    assert!(
        tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .is_err()
    );
    assert!(dropped.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(x.calls().len(), 1);
    assert!(x.writes().is_empty());
}

#[tokio::test]
async fn persists_pcs_cookies_and_sends_them_on_resumed_cloudkit_reads() {
    let first = client(
        |request, _| {
            if path(request).ends_with("/requestWebAccessState") {
                ok(json!({"isICDRSDisabled": true, "isDeviceConsentedForPCS": true}))
            } else {
                json_response(
                    200,
                    json!({"status": "success"}),
                    &[(
                        "set-cookie",
                        "X-APPLE-WEBAUTH-PCS-Cloudkit=fixture-cookie; Domain=icloud.com; Path=/; Secure; HttpOnly",
                    )],
                )
            }
        },
        saved(),
    );
    assert_eq!(
        first.instance.request_pcs().await.unwrap(),
        ProtectedAccess::Ready
    );
    let last = first.writes().last().cloned().unwrap();
    let resumed = client(|_, _| ok(json!({"zones": [{"records": []}]})), last);
    resumed
        .instance
        .cloudkit(CkPath::ChangesZone, json!({"zones": []}))
        .await
        .unwrap();
    let cookie = resumed.calls()[0]
        .header("Cookie")
        .unwrap_or_default()
        .to_owned();
    assert!(
        cookie.contains("X-APPLE-WEBAUTH-PCS-Cloudkit=fixture-cookie"),
        "{cookie}"
    );
}

#[tokio::test]
async fn handles_pcs_response_success_and_unknown() {
    for (message, ready) in [("success", true), ("unknown", false)] {
        let x = client(
            move |request, _| {
                ok(if path(request).ends_with("/requestWebAccessState") {
                    json!({"isICDRSDisabled": true, "isDeviceConsentedForPCS": true})
                } else if message == "success" {
                    json!({"status": "success"})
                } else {
                    json!({"message": message})
                })
            },
            saved(),
        );
        let result = x.instance.request_pcs().await;
        assert_eq!(result.is_ok(), ready);
        if ready {
            assert_eq!(result.unwrap(), ProtectedAccess::Ready);
        }
        let paths: Vec<String> = x.calls().iter().map(path).collect();
        assert_eq!(
            paths,
            [
                "/setup/ws/1/requestWebAccessState",
                "/setup/ws/1/requestPCS"
            ]
        );
    }
}

#[tokio::test]
async fn requires_a_confirmed_pcs_notification() {
    for sent in [true, false] {
        let x = client(
            move |request, _| {
                ok(if path(request).ends_with("/requestWebAccessState") {
                    json!({"isICDRSDisabled": true, "isDeviceConsentedForPCS": false})
                } else {
                    json!({"isDeviceConsentNotificationSent": sent})
                })
            },
            saved(),
        );
        assert_eq!(x.instance.request_pcs().await.is_ok(), sent);
        assert_eq!(x.calls().len(), 2);
    }
}

#[tokio::test]
async fn uses_upstream_service_headers_for_setup_and_cloudkit_only() {
    let x = client(
        |request, _| {
            ok(if path(request).ends_with("/validate") {
                json!({
                    "dsInfo": {"hsaVersion": 2},
                    "hsaTrustedBrowser": true,
                    "webservices": {"ckdatabasews": {"url": "https://ckdatabasews.icloud.com"}},
                })
            } else {
                json!({"zones": [{"records": []}]})
            })
        },
        saved(),
    );
    assert!(x.instance.verify_session().await.unwrap());
    x.instance
        .cloudkit(CkPath::ChangesZone, json!({"zones": []}))
        .await
        .unwrap();
    let calls = x.calls();
    assert_eq!(calls.len(), 2);
    for request in calls {
        assert_eq!(request.header("User-Agent"), Some("python-requests/2.31.0"));
        assert_eq!(request.header("Referer"), Some("https://www.icloud.com/"));
    }
}

#[tokio::test]
async fn rejects_a_non_apple_cloudkit_host_before_sending_a_request() {
    let mut session = saved();
    session["ckBaseUrl"] =
        json!("https://attacker.example/database/1/com.apple.reminders/production/private");
    let x = client(|_, _| ok(json!({})), session);
    assert!(
        x.instance
            .cloudkit(CkPath::ChangesZone, json!({"zones": []}))
            .await
            .is_err()
    );
    assert!(x.calls().is_empty());
}

#[tokio::test]
async fn refuses_a_redirect_without_forwarding_session_cookies() {
    let x = client(
        |_, _| AppleResponse::new(302, &[("location", "https://attacker.example/collect")], ""),
        saved(),
    );
    let error = x
        .instance
        .cloudkit(CkPath::ChangesZone, json!({"zones": []}))
        .await
        .unwrap_err();
    assert_eq!(error.kind, AppleErrorKind::UnsupportedProtocol);
    assert_eq!(x.calls().len(), 1);
}

#[tokio::test]
async fn retries_cloudkit_once_after_refreshing_the_token_session() {
    let x = client(
        |request, index| {
            if path(request).ends_with("/accountLogin") {
                return ok(json!({
                    "dsInfo": {"hsaVersion": 2},
                    "webservices": {"ckdatabasews": {"url": "https://ckdatabasews.icloud.com"}},
                }));
            }
            json_response(
                if index == 1 { 401 } else { 200 },
                json!({"zones": [{"records": []}]}),
                &[],
            )
        },
        saved(),
    );
    let result = x
        .instance
        .cloudkit(CkPath::ChangesZone, json!({"zones": []}))
        .await
        .unwrap();
    assert_eq!(result, json!({"zones": [{"records": []}]}));
    assert_eq!(x.calls().len(), 3);
    assert!(!x.writes().is_empty());
}

#[tokio::test]
async fn never_retries_a_rejected_modify_request() {
    let x = client(
        |_, _| {
            json_response(
                401,
                json!({"error": {"errorCode": "AUTHENTICATION_FAILED"}}),
                &[],
            )
        },
        saved(),
    );
    assert!(
        x.instance
            .cloudkit(CkPath::RecordsModify, json!({"operations": []}))
            .await
            .is_err()
    );
    assert_eq!(x.calls().len(), 1);
}

#[tokio::test]
async fn never_retries_mutation_queries_after_unauthorized_response() {
    for record_type in [
        "CompleteRecurringReminder",
        "createTreeDeletion",
        "FutureMutation",
    ] {
        let x = client(|_, _| json_response(401, json!({}), &[]), saved());
        assert!(
            x.instance
                .cloudkit(
                    CkPath::RecordsQuery,
                    json!({"query": {"recordType": record_type}})
                )
                .await
                .is_err()
        );
        assert_eq!(x.calls().len(), 1, "{record_type}");
    }
}

#[tokio::test]
async fn can_refresh_authentication_for_the_known_read_only_reminder_list_query() {
    let x = client(
        |request, index| {
            if path(request).ends_with("/accountLogin") {
                return ok(json!({
                    "dsInfo": {"hsaVersion": 2},
                    "webservices": {"ckdatabasews": {"url": "https://ckdatabasews.icloud.com"}},
                }));
            }
            json_response(
                if index == 1 { 401 } else { 200 },
                json!({"records": []}),
                &[],
            )
        },
        saved(),
    );
    let result = x
        .instance
        .cloudkit(
            CkPath::RecordsQuery,
            json!({"query": {"recordType": "reminderList"}}),
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"records": []}));
    assert_eq!(x.calls().len(), 3);
}

#[tokio::test]
async fn classifies_a_rate_limited_session_check_without_starting_sign_in() {
    let x = client(|_, _| json_response(503, json!({}), &[]), saved());
    let error = x.instance.verify_session().await.unwrap_err();
    assert_eq!(error.kind, AppleErrorKind::RateLimited);
    assert_eq!(x.calls().len(), 1);
}

#[tokio::test]
async fn treats_apples_421_as_an_expired_session_without_starting_sign_in() {
    let x = client(|_, _| json_response(421, json!({}), &[]), saved());
    assert!(!x.instance.verify_session().await.unwrap());
    let calls = x.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(path(&calls[0]), "/setup/ws/1/validate");
}

#[tokio::test]
async fn allows_an_explicit_sign_in_after_saved_session_validation_returns_421() {
    let x = client(
        |request, _| {
            json_response(
                if path(request).ends_with("/validate") {
                    421
                } else {
                    429
                },
                json!({}),
                &[],
            )
        },
        saved(),
    );
    let error = x.instance.begin_sign_in().await.unwrap_err();
    assert_eq!(error.status, Some(429));
    let paths: Vec<String> = x.calls().iter().map(path).collect();
    assert_eq!(
        paths,
        ["/setup/ws/1/validate", "/appleauth/auth/signin/init"]
    );
}

fn srp_challenge() -> Value {
    json!({
        "protocol": "s2k",
        "iteration": 1,
        "salt": "AQEBAQEBAQEBAQEBAQEBAQ==",
        "b": "Ag==",
        "c": "opaque-challenge",
    })
}

const AUTH_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

#[tokio::test]
async fn handles_push_status_with_challenge_and_options_status() {
    let cases: [(u16, bool, u16); 9] = [
        (200, true, 200),
        (202, true, 200),
        (405, true, 200),
        (405, false, 200),
        (200, true, 405),
        (401, true, 200),
        (429, true, 200),
        (500, true, 200),
        (503, true, 200),
    ];
    for (push_status, has_challenge, options_status) in cases {
        let x = client(
            move |request, _| {
                let p = path(request);
                let challenge =
                    |scnt: &'static str, id: &'static str| -> Vec<(&'static str, &'static str)> {
                        if has_challenge {
                            vec![("scnt", scnt), ("x-apple-id-session-id", id)]
                        } else {
                            Vec::new()
                        }
                    };
                if p.ends_with("/verify/trusteddevice/securitycode") {
                    if request.method == Method::POST {
                        return json_response(400, json!({}), &[]);
                    }
                    return json_response(
                        push_status,
                        json!({}),
                        &challenge("push-scnt", "push-id"),
                    );
                }
                if p.ends_with("/signin/init") {
                    return json_response(
                        200,
                        srp_challenge(),
                        &challenge("fresh-scnt", "fresh-id"),
                    );
                }
                if p.ends_with("/signin/complete") {
                    return json_response(
                        409,
                        json!({}),
                        &[("x-apple-session-token", "new-session")],
                    );
                }
                if p.ends_with("/accountLogin") {
                    return ok(json!({"dsInfo": {"hsaVersion": 2}, "hsaTrustedBrowser": false}));
                }
                if p == "/appleauth/auth" {
                    return json_response(
                        options_status,
                        json!({}),
                        &challenge("options-scnt", "options-id"),
                    );
                }
                ok(json!({}))
            },
            json!({"clientId": "auth-test"}),
        );
        let label =
            format!("push {push_status} challenge {has_challenge} options {options_status}");
        let result = x.instance.begin_sign_in().await;
        if options_status == 200
            && (matches!(push_status, 200 | 202) || (push_status == 405 && has_challenge))
        {
            assert_eq!(result, Ok(AppleBeginResult::MfaRequired), "{label}");
        } else {
            let error = result.unwrap_err();
            let expected = if options_status != 200 {
                options_status
            } else {
                push_status
            };
            assert_eq!(error.status, Some(expected), "{label}");
            assert_eq!(
                error.operation,
                if options_status != 200 {
                    "MFA options"
                } else {
                    "MFA push"
                },
                "{label}"
            );
        }
        let calls = x.calls();
        let complete = calls
            .iter()
            .find(|c| path(c).ends_with("/signin/complete"))
            .unwrap();
        if has_challenge {
            assert_eq!(complete.header("scnt"), Some("fresh-scnt"), "{label}");
            assert_eq!(
                complete.header("X-Apple-ID-Session-Id"),
                Some("fresh-id"),
                "{label}"
            );
        }
        let pushes: Vec<&AppleRequest> = calls
            .iter()
            .filter(|c| path(c).ends_with("/verify/trusteddevice/securitycode"))
            .collect();
        assert_eq!(pushes.len(), usize::from(options_status == 200), "{label}");
        if options_status == 200 {
            assert_eq!(pushes[0].method, Method::PUT);
            assert!(pushes[0].body.is_none());
            if has_challenge {
                assert_eq!(pushes[0].header("scnt"), Some("options-scnt"));
                assert_eq!(
                    pushes[0].header("X-Apple-ID-Session-Id"),
                    Some("options-id")
                );
            }
        }
        if matches!(push_status, 200 | 202) && options_status == 200 {
            let submitted = x.instance.submit_code("123456").await.unwrap_err();
            assert_eq!(submitted.status, Some(400));
            let calls = x.calls();
            let last = calls.last().unwrap();
            assert_eq!(
                path(last),
                "/appleauth/auth/verify/trusteddevice/securitycode"
            );
            assert_eq!(last.method, Method::POST);
            assert_eq!(last.header("scnt"), Some("push-scnt"));
            assert_eq!(last.header("X-Apple-ID-Session-Id"), Some("push-id"));
            assert_eq!(
                last.body.as_deref(),
                Some(r#"{"securityCode":{"code":"123456"}}"#)
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|c| path(c).ends_with("/signin/init"))
                    .count(),
                1
            );
        }
        for request in x.calls() {
            if request.url.host_str() != Some("idmsa.apple.com") {
                continue;
            }
            assert_eq!(request.header("User-Agent"), Some(AUTH_UA));
            assert_eq!(
                request.header("Referer"),
                Some(if path(&request).contains("/signin/") {
                    "https://www.icloud.com/"
                } else {
                    "https://idmsa.apple.com"
                })
            );
        }
    }
}

#[tokio::test]
async fn limits_account_login_mfa_fallback_after_srp_and_setup_status() {
    let cases: [(u16, u16, bool); 11] = [
        (409, 401, true),
        (409, 403, true),
        (409, 421, true),
        (200, 401, false),
        (200, 403, false),
        (200, 421, false),
        (409, 451, false),
        (409, 429, false),
        (409, 500, false),
        (409, 503, false),
        (409, 200, false),
    ];
    for (complete_status, setup_status, expects_mfa) in cases {
        let x = client(
            move |request, _| {
                let p = path(request);
                if p.ends_with("/signin/init") {
                    return ok(srp_challenge());
                }
                if p.ends_with("/signin/complete") {
                    return json_response(
                        complete_status,
                        json!({}),
                        &[("x-apple-session-token", "new-session")],
                    );
                }
                if p.ends_with("/accountLogin") {
                    let body = if setup_status == 200 {
                        json!({"termsUpdateNeeded": true})
                    } else {
                        json!({})
                    };
                    return json_response(setup_status, body, &[]);
                }
                ok(json!({}))
            },
            json!({"clientId": "auth-test"}),
        );
        let label = format!("complete {complete_status} setup {setup_status}");
        let result = x.instance.begin_sign_in().await;
        if expects_mfa {
            assert_eq!(result, Ok(AppleBeginResult::MfaRequired), "{label}");
        } else {
            let expected = if setup_status == 200 {
                451
            } else {
                setup_status
            };
            assert_eq!(result.unwrap_err().status, Some(expected), "{label}");
        }
        let calls = x.calls();
        assert_eq!(
            calls.iter().any(|c| path(c) == "/appleauth/auth"),
            expects_mfa,
            "{label}"
        );
        assert_eq!(
            calls
                .iter()
                .any(|c| path(c).ends_with("/verify/trusteddevice/securitycode")),
            expects_mfa,
            "{label}"
        );
    }
}

#[tokio::test]
async fn verifies_code_validity_token_trust_and_account() {
    // (status, valid, token, trust status, account status, ready)
    let cases: [(u16, Option<bool>, bool, u16, u16, bool); 10] = [
        (200, Some(true), true, 200, 200, true),
        (204, None, true, 204, 200, true),
        (409, Some(true), true, 200, 200, true),
        (409, Some(true), false, 200, 200, false),
        (409, Some(false), true, 200, 200, false),
        (409, None, true, 200, 200, false),
        (200, Some(false), true, 200, 200, false),
        (202, Some(true), true, 200, 200, false),
        (409, Some(true), true, 403, 200, false),
        (409, Some(true), true, 200, 451, false),
    ];
    for (status, valid, token, trust_status, account_status, ready) in cases {
        let calls_seen = Arc::new(AtomicUsize::new(0));
        let seen = calls_seen.clone();
        let mut initial = saved();
        initial["scnt"] = json!("challenge");
        initial["sessionId"] = json!("challenge-id");
        let x = client(
            move |request, _| {
                seen.fetch_add(1, Ordering::SeqCst);
                let p = path(request);
                if p.ends_with("/securitycode") {
                    let mut headers = vec![
                        ("scnt", "verified-scnt"),
                        ("x-apple-id-session-id", "verified-id"),
                    ];
                    if token {
                        headers.push(("x-apple-session-token", "verified-token"));
                    }
                    if status == 204 {
                        return AppleResponse::new(status, &headers, "");
                    }
                    let body = match valid {
                        Some(v) => json!({"securityCode": {"valid": v}}),
                        None => json!({"securityCode": {}}),
                    };
                    return json_response(status, body, &headers);
                }
                if p.ends_with("/2sv/trust") {
                    return AppleResponse::new(
                        trust_status,
                        &[("x-apple-twosv-trust-token", "verified-trust")],
                        "",
                    );
                }
                if p.ends_with("/accountLogin") {
                    return json_response(
                        account_status,
                        json!({
                            "dsInfo": {"hsaVersion": 2, "dsid": "fixture"},
                            "hsaTrustedBrowser": true,
                            "webservices": {"ckdatabasews": {"url": "https://ckdatabasews.icloud.com"}},
                        }),
                        &[],
                    );
                }
                panic!("Unexpected fixture request {p}");
            },
            initial,
        );
        let label = format!(
            "{status} valid={valid:?} token={token} trust {trust_status} account {account_status}"
        );
        let result = x.instance.submit_code("123456").await;
        assert_eq!(result.is_ok(), ready, "{label}: {result:?}");
        let passed = valid != Some(false)
            && (status == 200 || status == 204 || (status == 409 && valid == Some(true) && token));
        let calls = x.calls();
        let expected = if !passed {
            1
        } else if trust_status == 403 {
            2
        } else {
            3
        };
        assert_eq!(calls.len(), expected, "{label}");
        if passed {
            assert_eq!(calls[1].header("scnt"), Some("verified-scnt"), "{label}");
            assert_eq!(
                calls[1].header("X-Apple-ID-Session-Id"),
                Some("verified-id"),
                "{label}"
            );
        }
        if passed && trust_status != 403 {
            let body: Value = serde_json::from_str(calls[2].body.as_deref().unwrap()).unwrap();
            assert_eq!(body["dsWebAuthToken"], "verified-token", "{label}");
            assert_eq!(body["trustToken"], "verified-trust", "{label}");
        }
    }
}

#[tokio::test]
async fn never_treats_code_verification_405_as_successful_authentication() {
    let x = client(
        |_, _| json_response(405, json!({}), &[]),
        json!({"clientId": "auth-test", "scnt": "challenge", "sessionId": "challenge-id"}),
    );
    let error = x.instance.submit_code("123456").await.unwrap_err();
    assert_eq!(error.operation, "MFA verify");
    assert_eq!(error.status, Some(405));
    let calls = x.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        path(&calls[0]),
        "/appleauth/auth/verify/trusteddevice/securitycode"
    );
}

#[tokio::test]
async fn drops_a_stale_persisted_challenge_before_retrying_srp_init() {
    let inits = Arc::new(AtomicUsize::new(0));
    let count = inits.clone();
    let x = client(
        move |request, _| {
            let p = path(request);
            if p.ends_with("/signin/init") {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    return json_response(409, json!({}), &[]);
                }
                return ok(srp_challenge());
            }
            if p.ends_with("/signin/complete") {
                return json_response(200, json!({}), &[("x-apple-session-token", "new-session")]);
            }
            if p.ends_with("/accountLogin") {
                return ok(json!({
                    "dsInfo": {"hsaVersion": 2},
                    "hsaTrustedBrowser": true,
                    "webservices": {"ckdatabasews": {"url": "https://ckdatabasews.icloud.com"}},
                }));
            }
            ok(json!({}))
        },
        json!({"clientId": "auth-test", "scnt": "old-scnt", "sessionId": "old-id"}),
    );
    assert_eq!(
        x.instance.begin_sign_in().await,
        Ok(AppleBeginResult::Ready)
    );
    let calls = x.calls();
    let inits: Vec<&AppleRequest> = calls
        .iter()
        .filter(|c| path(c).ends_with("/signin/init"))
        .collect();
    assert_eq!(inits.len(), 2);
    assert_eq!(inits[0].header("scnt"), Some("old-scnt"));
    assert_eq!(inits[1].header("scnt"), None);
}

#[tokio::test]
async fn rejects_an_mfa_code_without_trusting_the_browser() {
    let mut initial = saved();
    initial["scnt"] = json!("scnt");
    initial["sessionId"] = json!("id");
    let x = client(
        |_, _| json_response(401, json!({"serviceErrors": [{"code": -21669}]}), &[]),
        initial,
    );
    assert!(x.instance.submit_code("123456").await.is_err());
    assert_eq!(x.calls().len(), 1);
}

#[tokio::test]
async fn reports_terms_without_accepting_them_or_exposing_response_contents() {
    let x = client(
        |_, _| ok(json!({"termsUpdateNeeded": true, "secret": "never-log-this"})),
        saved(),
    );
    let error = x.instance.verify_session().await.unwrap_err();
    assert_eq!(error.kind, AppleErrorKind::TermsRequired);
    assert!(!format!("{error:?} {error}").contains("never-log-this"));
    assert_eq!(x.calls().len(), 1);
}

#[tokio::test]
async fn record_mode_sends_reads_but_never_mutations() {
    let transport = Arc::new(Fake {
        responder: Box::new(|_, _| Box::pin(async { Ok(ok(json!({"records": []}))) })),
        calls: Mutex::new(Vec::new()),
    });
    let storage = Arc::new(Storage {
        initial: Mutex::new(saved()),
        writes: Mutex::new(Vec::new()),
    });
    let instance = AppleRemindersClient::new(AppleClientOptions {
        account: "test@example.com".into(),
        password: "test-password".into(),
        storage,
        transport: transport.clone(),
        clock: omni_testkit::test_clock(1_800_000_000_000),
        timeout: None,
        side_effects: SideEffectMode::Record,
    });
    instance
        .cloudkit(CkPath::RecordsLookup, json!({"records": []}))
        .await
        .unwrap();
    assert!(
        instance
            .cloudkit(CkPath::RecordsModify, json!({"operations": []}))
            .await
            .is_err()
    );
    assert!(
        instance
            .cloudkit(
                CkPath::RecordsQuery,
                json!({"query": {"recordType": "CompleteRecurringReminder"}})
            )
            .await
            .is_err()
    );
    assert!(instance.begin_sign_in().await.is_err());
    let paths: Vec<String> = transport.calls.lock().unwrap().iter().map(path).collect();
    assert_eq!(
        paths,
        [
            "/database/1/com.apple.reminders/production/private/records/lookup",
            "/setup/ws/1/validate",
        ]
    );
}

#[test]
fn session_round_trips_the_stored_shape() {
    let session: AppleSession = serde_json::from_value(saved()).unwrap();
    assert_eq!(serde_json::to_value(&session).unwrap(), saved());
    assert!(serde_json::from_value::<AppleSession>(json!({"scnt": "x"})).is_err());
}

/// Stored empty strings count as absent, so an empty session
/// token validates nothing and empty challenge headers are never sent.
#[tokio::test]
async fn treats_stored_empty_session_fields_as_absent() {
    let mut stored = saved();
    stored["sessionToken"] = json!("");
    stored["ckBaseUrl"] = json!("");
    let x = client(|_, _| ok(json!({})), stored.clone());
    assert!(!x.instance.verify_session().await.unwrap());
    let error = x
        .instance
        .cloudkit(CkPath::RecordsLookup, json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.reason, "session not ready");
    assert!(x.calls().is_empty());

    stored["scnt"] = json!("");
    stored["sessionId"] = json!("");
    let x = client(|_, _| ok(json!({})), stored);
    let error = x.instance.submit_code("123456").await.unwrap_err();
    assert_eq!(error.reason, "MFA challenge missing");
    assert!(x.calls().is_empty());
}
