//! Bounded CalDAV HTTP.
//!
//! Every request has a 15 s timeout covering headers and body, and dropping
//! the future aborts it. Bodies are read with a byte cap. PROPFIND follows
//! redirects manually (at most 5) and forwards credentials only over HTTPS and
//! only between trusted iCloud CalDAV hosts.

use std::time::Duration;

use base64::Engine as _;
use omni_http::{HttpClient, HttpError, Method, RedirectRule, StatusCode, Url};

use crate::error::CaldavError;

pub const MAX_REDIRECTS: usize = 5;
pub const CALDAV_REQUEST_TIMEOUT: Duration = Duration::from_millis(15_000);
pub const CALDAV_XML_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const CALDAV_ERROR_MAX_BYTES: usize = 64 * 1024;

/// One CalDAV response, body already read under its cap.
#[derive(Clone, Debug)]
pub struct CaldavResponse {
    pub status: u16,
    /// The canonical reason phrase (`statusText`).
    pub reason: String,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

impl CaldavResponse {
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// `pXX-caldav.icloud.com` or the `caldav.icloud.com` root.
pub fn is_icloud_caldav_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "caldav.icloud.com" {
        return true;
    }
    host.strip_prefix('p')
        .and_then(|rest| rest.strip_suffix("-caldav.icloud.com"))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// Rejects URLs that could disclose the provider password to another origin.
pub fn assert_trusted_caldav_url(url: &str) -> Result<Url, CaldavError> {
    let parsed = Url::parse(url).map_err(|e| {
        CaldavError::new(
            "validate iCloud CalDAV URL",
            format!("Invalid URL: {e}"),
            false,
        )
    })?;
    let trusted =
        parsed.scheme() == "https" && parsed.host_str().is_some_and(is_icloud_caldav_host);
    if !trusted {
        return Err(CaldavError::new(
            "validate iCloud CalDAV URL",
            format!("Untrusted iCloud CalDAV URL: {}", origin(&parsed)),
            false,
        ));
    }
    Ok(parsed)
}

fn is_safe_redirect(from: &Url, to: &Url) -> bool {
    if to.scheme() != "https" {
        return false;
    }
    if from.host_str() == to.host_str() {
        return true;
    }
    from.host_str().is_some_and(is_icloud_caldav_host)
        && to.host_str().is_some_and(is_icloud_caldav_host)
}

/// `Basic base64(user:password)`.
pub fn basic_auth(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    )
}

fn too_large(operation: &str, limit: usize) -> CaldavError {
    CaldavError::new(
        operation,
        format!("response exceeds the {limit}-byte limit"),
        false,
    )
}

/// One bounded request. Redirects are returned, not followed.
pub(crate) async fn request(
    http: &HttpClient,
    method: Method,
    url: &Url,
    headers: &[(&'static str, &str)],
    body: Option<String>,
    operation: &str,
    max_bytes: usize,
) -> Result<CaldavResponse, CaldavError> {
    let mut builder = http
        .request(method, url.clone())
        .timeout(CALDAV_REQUEST_TIMEOUT)
        .redirect(RedirectRule::None);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    if let Some(body) = body {
        builder = builder.body(body);
    }
    let response = builder
        .send_bounded(max_bytes)
        .await
        .map_err(|error| match error {
            HttpError::TooLarge { limit } => too_large(operation, limit),
            HttpError::InvalidUrl(message) | HttpError::Blocked(message) => {
                CaldavError::new(operation, message, false)
            }
            other => CaldavError::new(operation, other.to_string(), true),
        })?;
    let status = response.status;
    Ok(CaldavResponse {
        status: status.as_u16(),
        reason: reason_phrase(status),
        location: response
            .headers
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        body: response.body.to_vec(),
    })
}

fn reason_phrase(status: StatusCode) -> String {
    status.canonical_reason().unwrap_or_default().to_owned()
}

/// An error response's text, refusing bodies over [`CALDAV_ERROR_MAX_BYTES`].
pub(crate) fn error_text(
    response: &CaldavResponse,
    operation: &str,
) -> Result<String, CaldavError> {
    if response.body.len() > CALDAV_ERROR_MAX_BYTES {
        return Err(too_large(operation, CALDAV_ERROR_MAX_BYTES));
    }
    Ok(response.text())
}

/// A PROPFIND answer: the multistatus XML and the URL that finally answered
/// (relative hrefs resolve against it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropfindResponse {
    pub xml: String,
    pub url: Url,
}

/// Issues a PROPFIND, following redirects manually: iCloud's discovery chain
/// redirects from `caldav.icloud.com` to a per-account `pXX-caldav.icloud.com`
/// shard.
pub async fn propfind(
    http: &HttpClient,
    url: &Url,
    auth_header: &str,
    depth: &str,
    body: &str,
) -> Result<PropfindResponse, CaldavError> {
    let method = Method::from_bytes(b"PROPFIND")
        .map_err(|e| CaldavError::new("CalDAV PROPFIND", e.to_string(), false))?;
    let mut current = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let response = request(
            http,
            method.clone(),
            &current,
            &[
                ("Content-Type", "application/xml; charset=utf-8"),
                ("Authorization", auth_header),
                ("Depth", depth),
            ],
            Some(body.to_owned()),
            "CalDAV PROPFIND",
            CALDAV_XML_MAX_BYTES,
        )
        .await?;
        if (300..400).contains(&response.status) {
            let Some(location) = response.location.as_deref() else {
                return Err(CaldavError::new(
                    "CalDAV PROPFIND redirect",
                    format!("missing Location ({current})"),
                    false,
                ));
            };
            let next = current.join(location).map_err(|e| {
                CaldavError::new(
                    "CalDAV PROPFIND redirect",
                    format!("invalid Location {location:?}: {e}"),
                    false,
                )
            })?;
            if !is_safe_redirect(&current, &next) {
                return Err(CaldavError::new(
                    "CalDAV PROPFIND redirect",
                    format!(
                        "Refusing to forward CalDAV credentials across redirect ({} -> {})",
                        origin(&current),
                        origin(&next)
                    ),
                    false,
                ));
            }
            current = next;
            continue;
        }
        if !response.is_ok() {
            let text = error_text(&response, "read CalDAV PROPFIND error response")?;
            return Err(CaldavError::new(
                "CalDAV PROPFIND",
                format!(
                    "{} {} ({current})\n{text}",
                    response.status, response.reason
                ),
                response.status >= 500,
            )
            .with_status(response.status));
        }
        let xml = response.text();
        if !xml.trim_start().starts_with('<') {
            return Err(CaldavError::new(
                "decode CalDAV XML",
                "CalDAV response is not XML",
                false,
            ));
        }
        return Ok(PropfindResponse { xml, url: current });
    }
    Err(CaldavError::new(
        "CalDAV PROPFIND",
        format!("exceeded {MAX_REDIRECTS} redirects ({url})"),
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icloud_hosts() {
        assert!(is_icloud_caldav_host("caldav.icloud.com"));
        assert!(is_icloud_caldav_host("p42-caldav.icloud.com"));
        assert!(is_icloud_caldav_host("P03-CALDAV.icloud.com"));
        assert!(!is_icloud_caldav_host("p-caldav.icloud.com"));
        assert!(!is_icloud_caldav_host("px1-caldav.icloud.com"));
        assert!(!is_icloud_caldav_host("icloud.com.attacker.example"));
    }

    #[test]
    fn basic_auth_matches_buffer_encoding() {
        assert_eq!(basic_auth("user", "pa:ss"), "Basic dXNlcjpwYTpzcw==");
    }
}
