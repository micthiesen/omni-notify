//! Executor OAuth bearer validation through Better Auth's MCP session endpoint.

use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::{AdapterOptions, USER_AGENT};

const SESSION_PATH: &str = "/api/auth/mcp/get-session";
const SESSION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SESSION_BYTES: usize = 1024 * 1024;

/// The event owner bound to a validated bearer. Never carries session secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedOwner {
    pub owner: String,
    pub authorization: String,
    pub expires_at: Timestamp,
}

/// Executor's session endpoint failed or answered with a non-JSON body.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
#[error("Executor authentication unavailable")]
pub struct AuthUnavailable;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    user_id: String,
    client_id: String,
    access_token_expires_at: String,
}

/// `executor:` plus SHA-256 of the JSON array `[userId, clientId]`.
pub fn owner_id(user_id: &str, client_id: &str) -> String {
    // serde_json escapes strings exactly like JSON.stringify for valid UTF-8.
    let encoded = serde_json::Value::from(vec![user_id, client_id]).to_string();
    format!(
        "executor:{}",
        hex::encode(Sha256::digest(encoded.as_bytes()))
    )
}

/// `^Bearer ([A-Za-z0-9._~+/-]+=*)$`
pub fn is_bearer(authorization: &str) -> bool {
    let Some(token) = authorization.strip_prefix("Bearer ") else {
        return false;
    };
    let body = token.trim_end_matches('=');
    !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~+/-".contains(&b))
}

/// JS `Date.parse` for the forms Better Auth emits: RFC 3339/ISO 8601 with an
/// offset, an offset-less date-time (UTC, the container's zone) and a bare date
/// (UTC, as in JS).
pub fn parse_expiry(raw: &str) -> Option<Timestamp> {
    if let Ok(timestamp) = raw.parse::<Timestamp>() {
        return Some(timestamp);
    }
    if let Ok(datetime) = raw.parse::<jiff::civil::DateTime>() {
        return datetime
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp());
    }
    raw.parse::<jiff::civil::Date>()
        .ok()
        .and_then(|date| date.to_zoned(jiff::tz::TimeZone::UTC).ok())
        .map(|z| z.timestamp())
}

/// Better Auth's get-session does not check expiry, so the adapter must.
///
/// `Ok(None)` means unauthorized (missing or malformed bearer, unknown session,
/// expired token or a different Executor user); `Err` means Executor could not
/// answer.
pub async fn authenticate_oauth(
    http: &reqwest::Client,
    authorization: Option<&str>,
    options: &AdapterOptions,
    now: Timestamp,
) -> Result<Option<AuthenticatedOwner>, AuthUnavailable> {
    let Some(authorization) = authorization.filter(|value| is_bearer(value)) else {
        return Ok(None);
    };
    let url = options
        .executor_base_url
        .join(SESSION_PATH)
        .map_err(|_| AuthUnavailable)?;
    let response = http
        .get(url)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(SESSION_TIMEOUT)
        .send()
        .await
        .map_err(|_| AuthUnavailable)?;
    if !response.status().is_success() {
        return Err(AuthUnavailable);
    }
    let body = response.bytes().await.map_err(|_| AuthUnavailable)?;
    if body.len() > MAX_SESSION_BYTES {
        return Err(AuthUnavailable);
    }
    let raw: serde_json::Value = serde_json::from_slice(&body).map_err(|_| AuthUnavailable)?;
    let Ok(session) = serde_json::from_value::<Session>(raw) else {
        return Ok(None);
    };
    if session.user_id.is_empty() || session.client_id.is_empty() {
        return Ok(None);
    }
    let Some(expires_at) = parse_expiry(&session.access_token_expires_at) else {
        return Ok(None);
    };
    if expires_at <= now || session.user_id != options.allowed_user_id {
        return Ok(None);
    }
    Ok(Some(AuthenticatedOwner {
        owner: owner_id(&session.user_id, &session.client_id),
        authorization: authorization.to_owned(),
        expires_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_id_hashes_the_json_pair() {
        // node -e 'console.log(require("crypto").createHash("sha256")
        //   .update(JSON.stringify(["owner-user","client-id"])).digest("hex"))'
        assert_eq!(
            owner_id("owner-user", "client-id"),
            format!(
                "executor:{}",
                hex::encode(Sha256::digest(br#"["owner-user","client-id"]"#))
            )
        );
        assert_ne!(owner_id("a", "bc"), owner_id("ab", "c"));
        assert_eq!(
            serde_json::Value::from(vec!["\u{1}\"/é"]).to_string(),
            r#"["\u0001\"/é"]"#
        );
    }

    #[test]
    fn bearer_shape_matches_the_ts_pattern() {
        assert!(is_bearer("Bearer abc.DEF_~+/-=="));
        assert!(!is_bearer("Bearer "));
        assert!(!is_bearer("Bearer ="));
        assert!(!is_bearer("bearer abc"));
        assert!(!is_bearer("Bearer a b"));
        assert!(!is_bearer("Bearer a=b"));
    }

    #[test]
    fn expiry_parses_like_date_parse() {
        let iso = parse_expiry("2026-10-09T12:00:00.000Z").unwrap();
        assert_eq!(iso.as_millisecond(), 1_791_547_200_000);
        assert_eq!(parse_expiry("2026-10-09T12:00:00"), Some(iso));
        assert_eq!(
            parse_expiry("2026-10-09T14:00:00+02:00"),
            Some(iso),
            "offsets are honored"
        );
        assert!(parse_expiry("2026-10-09").is_some());
        assert_eq!(parse_expiry("soon"), None);
    }
}
