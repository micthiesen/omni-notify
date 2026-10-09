//! Public status and challenge controls; no account data.
//!
//! Every `/api/reminders/*` response is `no-store`. Requests must carry the exact
//! configured public host; POSTs need the exact configured Origin, a same-origin
//! `Sec-Fetch-Site` when present, `application/json`, and stay within per-path rate
//! limits. Without a valid HTTPS public origin the status route reports a
//! configuration-disabled feature and the controls answer 503.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::future::BoxFuture;
use omni_api::reminders::{
    CodeRequest, Phase, PublicStatus, Reason, StatusErrorResponse, StatusResponse,
};
use omni_core::clock::SharedClock;

use crate::service::RemindersService;

/// The administration seam (the TS `RemindersControl`); errors carry no details.
pub trait RemindersControl: Send + Sync {
    fn status(&self) -> BoxFuture<'_, Result<PublicStatus, ()>>;
    fn start_authentication(&self) -> BoxFuture<'_, Result<PublicStatus, ()>>;
    fn submit_code(&self, input: CodeRequest) -> BoxFuture<'_, Result<PublicStatus, ()>>;
    fn verify_access(&self) -> BoxFuture<'_, Result<PublicStatus, ()>>;
}

impl RemindersControl for RemindersService {
    fn status(&self) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        Box::pin(async move { Ok(RemindersService::status(self)) })
    }

    fn start_authentication(&self) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        Box::pin(async move {
            RemindersService::start_authentication(self)
                .await
                .map_err(|_| ())
        })
    }

    fn submit_code(&self, input: CodeRequest) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        Box::pin(async move {
            RemindersService::submit_code(self, &input.challenge_id, &input.code)
                .await
                .map_err(|_| ())
        })
    }

    fn verify_access(&self) -> BoxFuture<'_, Result<PublicStatus, ()>> {
        Box::pin(async move { Ok(RemindersService::verify_access(self).await) })
    }
}

const STATUS_PATH: &str = "/api/reminders/status";
const START_PATH: &str = "/api/reminders/auth/start";
const CODE_PATH: &str = "/api/reminders/auth/code";
const VERIFY_PATH: &str = "/api/reminders/auth/verify";
const CODE_BODY_LIMIT: usize = 512;

/// `(count, window)` per mutating path.
fn limit_for(path: &str) -> Option<(usize, i64)> {
    match path {
        START_PATH => Some((3, 5 * 60_000)),
        CODE_PATH | VERIFY_PATH => Some((10, 60_000)),
        _ => None,
    }
}

#[derive(Clone)]
struct RoutesState {
    control: Arc<dyn RemindersControl>,
    allowed_origin: Option<String>,
    allowed_host: Option<String>,
    attempts: Arc<Mutex<HashMap<&'static str, Vec<i64>>>>,
    clock: SharedClock,
}

/// `new URL(origin).host` (host plus a non-default port).
fn url_host(url: &url::Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

/// Accepts only an HTTPS origin without credentials, path, query or fragment.
fn allowed_origin(public_origin: Option<&str>) -> Option<(String, String)> {
    let parsed = url::Url::parse(public_origin?).ok()?;
    let valid = parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none();
    valid
        .then(|| (parsed.origin().ascii_serialization(), url_host(&parsed)))
        .and_then(|(origin, host)| Some((origin, host?)))
}

fn json_response<T: serde::Serialize>(status: StatusCode, body: &T) -> Response {
    (status, axum::Json(body)).into_response()
}

fn error(status: StatusCode, message: &str) -> Response {
    json_response(
        status,
        &StatusErrorResponse {
            error: message.to_owned(),
            status: None,
        },
    )
}

fn no_store(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
}

fn header_str(request: &Request, name: header::HeaderName) -> Option<&str> {
    request.headers().get(name).and_then(|v| v.to_str().ok())
}

async fn guard(State(state): State<RoutesState>, request: Request, next: Next) -> Response {
    let mut response = guard_inner(&state, request, next).await;
    no_store(&mut response);
    response
}

async fn guard_inner(state: &RoutesState, request: Request, next: Next) -> Response {
    let (Some(origin), Some(allowed_host)) = (&state.allowed_origin, &state.allowed_host) else {
        if request.method() == Method::GET && request.uri().path() == STATUS_PATH {
            return json_response(
                StatusCode::OK,
                &StatusResponse {
                    status: PublicStatus::new(false, Phase::Disabled)
                        .with_reason(Reason::Configuration),
                },
            );
        }
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Reminders administration is unavailable",
        );
    };
    let host = header_str(&request, header::HOST)
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(|a| a.as_str().to_owned()));
    if host.as_deref() != Some(allowed_host.as_str()) {
        return error(StatusCode::FORBIDDEN, "Forbidden");
    }
    if request.method() != Method::GET {
        let fetch_site = header_str(&request, HeaderName::from_static("sec-fetch-site"));
        if header_str(&request, header::ORIGIN) != Some(origin.as_str())
            || fetch_site.is_some_and(|site| !site.is_empty() && site != "same-origin")
        {
            return error(StatusCode::FORBIDDEN, "Forbidden");
        }
        let content_type = header_str(&request, header::CONTENT_TYPE)
            .and_then(|v| v.split(';').next())
            .map(str::trim);
        if content_type != Some("application/json") {
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Expected application/json",
            );
        }
        let path = request.uri().path().to_owned();
        if let Some((count, window)) = limit_for(&path) {
            let now = state.clock.now_ms();
            let key: &'static str = match path.as_str() {
                START_PATH => START_PATH,
                CODE_PATH => CODE_PATH,
                _ => VERIFY_PATH,
            };
            let limited = {
                let mut attempts = state
                    .attempts
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let recent = attempts.entry(key).or_default();
                recent.retain(|time| now - time < window);
                if recent.len() >= count {
                    true
                } else {
                    recent.push(now);
                    false
                }
            };
            if limited {
                let mut response = error(StatusCode::TOO_MANY_REQUESTS, "Too many requests");
                if let Ok(value) = HeaderValue::from_str(&(window / 1000).to_string()) {
                    response.headers_mut().insert(header::RETRY_AFTER, value);
                }
                return response;
            }
        }
    }
    next.run(request).await
}

async fn status_route(State(state): State<RoutesState>) -> Response {
    match state.control.status().await {
        Ok(status) => json_response(
            StatusCode::OK,
            &StatusResponse {
                status: public(status),
            },
        ),
        Err(()) => error(StatusCode::BAD_GATEWAY, "Reminders request failed"),
    }
}

/// `publicStatus`: drops an out-of-range HTTP status from the diagnostic.
fn public(mut status: PublicStatus) -> PublicStatus {
    if let Some(diagnostic) = status.diagnostic.as_mut() {
        diagnostic.http_status = diagnostic.http_status.filter(|s| (100..=599).contains(s));
    }
    status.challenge_id = status.challenge_id.filter(|c| !c.is_empty());
    status.challenge_expires_at = status.challenge_expires_at.filter(|e| *e != 0);
    status
}

async fn failed_with_status(state: &RoutesState) -> Response {
    match state.control.status().await {
        Ok(status) => {
            let code = if status.phase == Phase::RateLimited {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::BAD_GATEWAY
            };
            json_response(
                code,
                &StatusErrorResponse {
                    error: "Reminders request failed".into(),
                    status: Some(public(status)),
                },
            )
        }
        Err(()) => error(StatusCode::BAD_GATEWAY, "Reminders request failed"),
    }
}

async fn start_route(State(state): State<RoutesState>) -> Response {
    match state.control.start_authentication().await {
        Ok(status) => json_response(
            StatusCode::OK,
            &StatusResponse {
                status: public(status),
            },
        ),
        Err(()) => failed_with_status(&state).await,
    }
}

async fn verify_route(State(state): State<RoutesState>) -> Response {
    match state.control.verify_access().await {
        Ok(status) => json_response(
            StatusCode::OK,
            &StatusResponse {
                status: public(status),
            },
        ),
        Err(()) => failed_with_status(&state).await,
    }
}

/// `decodeJsonBody(c, codeSchema, 512)`.
async fn read_code(body: Body) -> Option<CodeRequest> {
    let bytes = axum::body::to_bytes(body, CODE_BODY_LIMIT).await.ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    let challenge_id = object.get("challengeId")?.as_str()?;
    let code = object.get("code")?.as_str()?;
    let challenge_ok = (1..=200).contains(&omni_core::js::utf16_len(challenge_id));
    let code_ok = code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit());
    (challenge_ok && code_ok).then(|| CodeRequest {
        challenge_id: challenge_id.to_owned(),
        code: code.to_owned(),
    })
}

async fn code_route(State(state): State<RoutesState>, request: Request) -> Response {
    let not_confirmed = || error(StatusCode::BAD_REQUEST, "Code submission was not confirmed");
    let Some(input) = read_code(request.into_body()).await else {
        return not_confirmed();
    };
    match state.control.submit_code(input).await {
        Ok(status) => json_response(
            StatusCode::OK,
            &StatusResponse {
                status: public(status),
            },
        ),
        Err(()) => not_confirmed(),
    }
}

/// The `/api/reminders/*` router.
pub fn reminders_router(
    control: Arc<dyn RemindersControl>,
    public_origin: Option<&str>,
    clock: SharedClock,
) -> Router {
    let (origin, host) = allowed_origin(public_origin).unzip();
    let state = RoutesState {
        control,
        allowed_origin: origin,
        allowed_host: host,
        attempts: Arc::new(Mutex::new(HashMap::new())),
        clock,
    };
    Router::new()
        .route(STATUS_PATH, get(status_route))
        .route(START_PATH, post(start_route))
        .route(CODE_PATH, post(code_route))
        .route(VERIFY_PATH, post(verify_route))
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}
