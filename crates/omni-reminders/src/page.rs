//! The server-rendered `/reminders` administration page.
//!
//! No framework and no WebAssembly, so the page keeps a strict CSP
//! (`script-src 'self'` without `'wasm-unsafe-eval'`). The page has no extra login;
//! it only exposes the bounded controls under `/api/reminders/*`.

use axum::Router;
use axum::http::{HeaderName, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

/// The `/reminders` CSP.
pub const REMINDERS_PAGE_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

const PAGE_HTML: &str = include_str!("../assets/reminders.html");
const PAGE_JS: &str = include_str!("../assets/reminders.js");

fn with_headers(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(REMINDERS_PAGE_CSP),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// `GET /reminders` and its script.
pub fn page_router() -> Router {
    Router::new()
        .route(
            "/reminders",
            get(|| async { with_headers("text/html; charset=utf-8", PAGE_HTML) }),
        )
        .route(
            "/reminders/app.js",
            get(|| async { with_headers("text/javascript; charset=utf-8", PAGE_JS) }),
        )
}
