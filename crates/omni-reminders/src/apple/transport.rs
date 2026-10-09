//! The HTTP seam for the Apple client.
//!
//! Apple-only exception approved by Michael (AGENTS.md): this client keeps
//! ioBroker's browser User-Agent for SRP/MFA, its service User-Agent for
//! setup/CloudKit, and the MFA-specific Referer. Those headers are set by
//! [`super::AppleRemindersClient`] per request and never leave this module's caller.

use std::time::Duration;

use bytes::Bytes;
use futures::future::BoxFuture;
use omni_http::{HttpClient, Method, RedirectRule, Url};

/// Response bodies above this are a protocol failure.
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// One outgoing Apple request.
#[derive(Clone, Debug, PartialEq)]
pub struct AppleRequest {
    pub method: Method,
    pub url: Url,
    /// Header names, spelled exactly as sent.
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl AppleRequest {
    /// A header by exact name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The raw response; redirects are returned, never followed.
#[derive(Clone, Debug, PartialEq)]
pub struct AppleResponse {
    pub status: u16,
    /// Lowercase names; repeated headers (Set-Cookie) appear once per value.
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl AppleResponse {
    pub fn new(status: u16, headers: &[(&str, &str)], body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), (*v).to_owned()))
                .collect(),
            body: body.into(),
        }
    }

    /// `Response.json(value, {status, headers})`.
    pub fn json(status: u16, value: &serde_json::Value, headers: &[(&str, &str)]) -> Self {
        let mut response = Self::new(status, headers, value.to_string());
        response
            .headers
            .push(("content-type".into(), "application/json".into()));
        response
    }

    /// `headers.get(name)`: repeated values joined with `", "`.
    pub fn header(&self, name: &str) -> Option<String> {
        let name = name.to_ascii_lowercase();
        let values: Vec<&str> = self
            .headers
            .iter()
            .filter(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    }

    /// `headers.getSetCookie()`.
    pub fn set_cookies(&self) -> impl Iterator<Item = &str> {
        self.headers
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .map(|(_, v)| v.as_str())
    }
}

/// Transport failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("network failure")]
    Network,
    #[error("Response too large")]
    TooLarge,
}

/// Sends one request. Implementations must not follow redirects.
pub trait AppleTransport: Send + Sync {
    fn send(&self, request: AppleRequest) -> BoxFuture<'_, Result<AppleResponse, TransportError>>;
}

/// The production transport over the shared reqwest client.
#[derive(Clone, Debug)]
pub struct HttpAppleTransport {
    http: HttpClient,
    /// Generous per-hop bound; the client applies the operation timeout.
    timeout: Duration,
}

impl HttpAppleTransport {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            timeout: Duration::from_secs(120),
        }
    }
}

impl AppleTransport for HttpAppleTransport {
    fn send(&self, request: AppleRequest) -> BoxFuture<'_, Result<AppleResponse, TransportError>> {
        Box::pin(async move {
            let mut builder = self
                .http
                .request(request.method, request.url)
                .redirect(RedirectRule::None)
                .timeout(self.timeout);
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let response = builder
                .send_bounded(MAX_RESPONSE_BYTES)
                .await
                .map_err(|error| match error {
                    omni_http::HttpError::TooLarge { .. } => TransportError::TooLarge,
                    _ => TransportError::Network,
                })?;
            let headers = response
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|v| (name.as_str().to_ascii_lowercase(), v.to_owned()))
                })
                .collect();
            Ok(AppleResponse {
                status: response.status.as_u16(),
                headers,
                body: response.body,
            })
        })
    }
}
