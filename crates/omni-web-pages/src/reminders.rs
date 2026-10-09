//! iCloud Reminders administration (`pages/RemindersPage.tsx`).
//!
//! The page talks only to the fixed same-origin `/api/reminders/*` routes,
//! without credentials or caching and refusing redirects. Requests go through
//! the [`RemindersTransport`] seam so the request shape and the response state
//! machine are testable without a browser; [`FetchTransport`] is the browser
//! implementation. What the page shows is derived by [`reminders_view`].

use std::future::Future;

use leptos::prelude::*;
use omni_api::reminders::{Phase, PublicStatus, Reason, StatusResponse};
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use web_sys::{RequestCache, RequestCredentials, RequestRedirect};

/// The four Reminders administration calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Status,
    Start,
    Code,
    Verify,
}

impl Operation {
    pub fn path(self) -> &'static str {
        match self {
            Operation::Status => "/api/reminders/status",
            Operation::Start => "/api/reminders/auth/start",
            Operation::Code => "/api/reminders/auth/code",
            Operation::Verify => "/api/reminders/auth/verify",
        }
    }
}

/// The body of a code submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeInput {
    pub challenge_id: String,
    pub code: String,
}

/// Everything `fetch` is called with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemindersHttpRequest {
    pub method: &'static str,
    pub path: &'static str,
    pub credentials: RequestCredentials,
    pub cache: RequestCache,
    pub redirect: RequestRedirect,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: Option<String>,
}

/// A received response; `body` is `None` when it could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemindersHttpResponse {
    pub status: u16,
    pub body: Option<String>,
}

/// The request could not be sent or no response arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportFailure;

/// Sends one Reminders request.
pub trait RemindersTransport {
    fn send(
        &self,
        request: RemindersHttpRequest,
    ) -> impl Future<Output = Result<RemindersHttpResponse, TransportFailure>>;
}

/// The request for `operation`: GET for status, JSON POST otherwise.
pub fn build_request(operation: Operation, input: Option<&CodeInput>) -> RemindersHttpRequest {
    let is_status = operation == Operation::Status;
    let body = (!is_status).then(|| match input {
        Some(input) => serde_json::json!({
            "challengeId": input.challenge_id,
            "code": input.code,
        })
        .to_string(),
        None => "{}".to_owned(),
    });
    RemindersHttpRequest {
        method: if is_status { "GET" } else { "POST" },
        path: operation.path(),
        credentials: RequestCredentials::Omit,
        cache: RequestCache::NoStore,
        redirect: RequestRedirect::Error,
        headers: if is_status {
            Vec::new()
        } else {
            vec![("Content-Type", "application/json")]
        },
        body,
    }
}

fn decode_status(body: Option<&str>) -> Option<PublicStatus> {
    serde_json::from_str::<StatusResponse>(body?)
        .ok()
        .map(|r| r.status)
}

/// `remindersRequest`: the decoded status, or a user-facing error message.
/// Failed start/verify calls that carry a non-authenticated status return
/// that status; other failures never echo the response body.
pub async fn reminders_request<T: RemindersTransport>(
    transport: &T,
    operation: Operation,
    input: Option<&CodeInput>,
) -> Result<PublicStatus, String> {
    let response = transport
        .send(build_request(operation, input))
        .await
        .map_err(|_| "Could not reach the Reminders service".to_owned())?;
    let ok = (200..300).contains(&response.status);
    if !ok
        && matches!(operation, Operation::Start | Operation::Verify)
        && let Some(status) = decode_status(response.body.as_deref())
        && status.phase != Phase::Authenticated
    {
        return Ok(status);
    }
    if !ok {
        return Err(if response.status == 429 {
            "Too many requests. Try again later.".to_owned()
        } else if operation == Operation::Code {
            "Code submission was not confirmed. Select Check access before trying again.".to_owned()
        } else {
            format!("Reminders request failed ({})", response.status)
        });
    }
    decode_status(response.body.as_deref()).ok_or_else(|| "Invalid Reminders response".to_owned())
}

/// `fetch` with the request's options; aborted if dropped mid-flight.
pub struct FetchTransport;

struct AbortOnDrop(Option<web_sys::AbortController>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(controller) = &self.0 {
            controller.abort();
        }
    }
}

impl RemindersTransport for FetchTransport {
    async fn send(
        &self,
        request: RemindersHttpRequest,
    ) -> Result<RemindersHttpResponse, TransportFailure> {
        use wasm_bindgen::JsCast as _;
        use wasm_bindgen_futures::JsFuture;

        let init = web_sys::RequestInit::new();
        init.set_method(request.method);
        init.set_credentials(request.credentials);
        init.set_cache(request.cache);
        init.set_redirect(request.redirect);
        let headers = web_sys::Headers::new().map_err(|_| TransportFailure)?;
        for (name, value) in &request.headers {
            headers.set(name, value).map_err(|_| TransportFailure)?;
        }
        init.set_headers(&headers);
        if let Some(body) = &request.body {
            init.set_body(&wasm_bindgen::JsValue::from_str(body));
        }
        let controller = web_sys::AbortController::new().ok();
        if let Some(controller) = &controller {
            init.set_signal(Some(&controller.signal()));
        }
        let mut guard = AbortOnDrop(controller);
        let response: web_sys::Response =
            JsFuture::from(window().fetch_with_str_and_init(request.path, &init))
                .await
                .map_err(|_| TransportFailure)?
                .dyn_into()
                .map_err(|_| TransportFailure)?;
        let status = response.status();
        let body = match response.text() {
            Ok(promise) => JsFuture::from(promise)
                .await
                .ok()
                .and_then(|v| v.as_string()),
            Err(_) => None,
        };
        // Completed: nothing left to abort.
        guard.0 = None;
        Ok(RemindersHttpResponse { status, body })
    }
}

/// `statusText`.
pub fn status_text(status: &PublicStatus) -> &'static str {
    let has_challenge = status
        .challenge_id
        .as_deref()
        .is_some_and(|c| !c.is_empty());
    match status.phase {
        Phase::Disabled => "Reminders monitoring is disabled on the server.",
        Phase::Authenticated => "Connected to iCloud Reminders.",
        Phase::AuthenticationNeeded if has_challenge => {
            "Enter the six-digit code shown on your trusted Apple device."
        }
        Phase::AuthenticationNeeded => "Sign in to connect iCloud Reminders.",
        Phase::AwaitingDeviceApproval if status.reason == Some(Reason::Pcs) => {
            "Apple sign-in succeeded, but protected Reminders data is not available yet. Select Check access to request access, approve any prompt on your trusted Apple device, then check access again. Keep Advanced Data Protection enabled."
        }
        Phase::AwaitingDeviceApproval => {
            "Approve the sign-in on your trusted Apple device, then check access."
        }
        Phase::TermsRequired => "Apple requires you to review account terms in its own interface.",
        Phase::RateLimited => "Apple has temporarily limited sign-in attempts. Try again later.",
        Phase::TransientOutage => "Apple is temporarily unavailable. Check access later.",
        Phase::UnsupportedProtocol => {
            "Apple sign-in or Reminders access returned an unsupported response. The diagnostic below identifies the failed step. Keep Advanced Data Protection enabled."
        }
    }
}

fn wire_name<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The intro paragraph shown in every state.
pub const INTRO: &str =
    "Connect your account to monitor Reminders. Keep Advanced Data Protection enabled.";
pub const HTTPS_REQUIRED: &str = "Open this page over HTTPS to administer Reminders.";
pub const LOADING: &str = "Loading connection status…";
pub const INVALID_CODE: &str = "Enter a six-digit code from your trusted device.";

/// What the page renders for a protocol and status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemindersView {
    /// Served over HTTPS; otherwise only the intro and [`HTTPS_REQUIRED`].
    pub secure: bool,
    /// The `role="status"` line.
    pub status_line: String,
    /// `Failed step: …` when the status carries a diagnostic.
    pub diagnostic: Option<String>,
    pub show_code_form: bool,
    pub show_start: bool,
    pub show_verify: bool,
}

/// Derives the page's visible parts (controls stay behind HTTPS).
pub fn reminders_view(secure: bool, status: Option<&PublicStatus>) -> RemindersView {
    if !secure {
        return RemindersView::default();
    }
    let Some(status) = status else {
        return RemindersView {
            secure,
            status_line: LOADING.to_owned(),
            ..RemindersView::default()
        };
    };
    let has_challenge = status
        .challenge_id
        .as_deref()
        .is_some_and(|c| !c.is_empty());
    let diagnostic = status.diagnostic.map(|d| {
        let http = d
            .http_status
            .filter(|s| *s != 0)
            .map(|s| format!(", Apple HTTP {s}"))
            .unwrap_or_default();
        format!(
            "Failed step: {} ({}{http}).",
            wire_name(d.stage),
            wire_name(d.category)
        )
    });
    RemindersView {
        secure,
        status_line: status_text(status).to_owned(),
        diagnostic,
        show_code_form: has_challenge && status.phase == Phase::AuthenticationNeeded,
        show_start: status.enabled
            && matches!(
                status.phase,
                Phase::AuthenticationNeeded
                    | Phase::UnsupportedProtocol
                    | Phase::TransientOutage
                    | Phase::RateLimited
            )
            && !has_challenge,
        show_verify: status.enabled && status.phase != Phase::Disabled,
    }
}

/// `/^[0-9]{6}$/`.
pub fn is_six_digit_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

fn is_secure() -> bool {
    window().location().protocol().is_ok_and(|p| p == "https:")
}

#[component]
pub fn RemindersPage() -> impl IntoView {
    let status = RwSignal::new(None::<PublicStatus>);
    let code = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let secure = is_secure();

    if secure {
        spawn_scoped(async move {
            match reminders_request(&FetchTransport, Operation::Status, None).await {
                Ok(next) => status.set(Some(next)),
                Err(message) => error.set(message),
            }
        });
    }

    let request = move |operation: Operation, input: Option<CodeInput>| {
        busy.set(true);
        error.set(String::new());
        spawn_detached(async move {
            match reminders_request(&FetchTransport, operation, input.as_ref()).await {
                Ok(next) => status.set(Some(next)),
                Err(message) => error.set(message),
            }
            busy.set(false);
        });
    };

    let submit_code = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let challenge_id = status
            .with_untracked(|s| s.as_ref().and_then(|s| s.challenge_id.clone()))
            .filter(|c| !c.is_empty());
        let submitted = code.get_untracked();
        code.set(String::new());
        match challenge_id {
            Some(challenge_id) if is_six_digit_code(&submitted) => request(
                Operation::Code,
                Some(CodeInput {
                    challenge_id,
                    code: submitted,
                }),
            ),
            _ => error.set(INVALID_CODE.to_owned()),
        }
    };

    let view_model = Memo::new(move |_| status.with(|s| reminders_view(secure, s.as_ref())));

    let controls = move || {
        let model = view_model.get();
        if !model.secure {
            return view! { <p role="alert">{HTTPS_REQUIRED}</p> }.into_any();
        }
        view! {
            <p role="status">{model.status_line.clone()}</p>
            {model.diagnostic.clone().map(|d| view! { <p role="alert">{d}</p> })}
            {model
                .show_code_form
                .then(|| {
                    view! {
                        <form on:submit=submit_code>
                            <label for="reminders-code">"Verification code"</label>
                            <input
                                id="reminders-code"
                                type="text"
                                inputmode="numeric"
                                autocomplete="one-time-code"
                                pattern="[0-9]{6}"
                                maxlength="6"
                                prop:value=move || code.get()
                                on:input=move |event| code.set(event_target_value(&event))
                            />
                            <button type="submit" disabled=move || busy.get()>
                                {move || if busy.get() { "Verifying…" } else { "Submit code" }}
                            </button>
                        </form>
                    }
                })}
            {model
                .show_start
                .then(|| {
                    view! {
                        <button type="button" disabled=move || busy.get() on:click=move |_| request(Operation::Start, None)>
                            "Start sign-in"
                        </button>
                    }
                })}
            {model
                .show_verify
                .then(|| {
                    view! {
                        <button type="button" disabled=move || busy.get() on:click=move |_| request(Operation::Verify, None)>
                            "Check access"
                        </button>
                    }
                })}
        }
        .into_any()
    };

    view! {
        <section class="reminders-panel" aria-label="iCloud Reminders">
            <h1>"iCloud Reminders"</h1>
            <p>{INTRO}</p>
            {controls}
            {move || error.with(|e| (!e.is_empty()).then(|| view! { <p role="alert">{e.clone()}</p> }))}
        </section>
    }
}
