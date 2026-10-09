//! Executor delegated-owner revalidation (`src/mcp/events/executorAuth.ts`).
//!
//! Resolves the expiry of an owner's delegated access token through the fixed
//! internal `OMNI_EVENTS_EXECUTOR_AUTH_URL`, or `None` when it does not validate
//! now. Only the expiry leaves this module: the session response may contain
//! credentials.

use std::time::Duration;

use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use omni_http::{HttpClient, Method, RedirectRule, Url};
use serde::Deserialize;
use serde_json::Value;

const MAX_SESSION_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Executor could not answer (network, 5xx, 429, malformed JSON).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Executor authorization check unavailable")]
pub struct ExecutorAuthError;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Invalid Executor session URL")]
pub struct InvalidSessionUrl;

/// `ExecutorEventAuthorizer`: when the owner's delegated token stops
/// validating (epoch ms), or `None` when it does not validate now.
pub trait EventAuthorizer: Send + Sync {
    fn authorize<'a>(
        &'a self,
        owner: &'a str,
        authorization: &'a str,
    ) -> BoxFuture<'a, Result<Option<i64>, ExecutorAuthError>>;
}

/// `executorOwnerId`: `executor:` + sha256hex(`JSON.stringify([userId, clientId])`).
pub fn executor_owner_id(user_id: &str, client_id: &str) -> String {
    let material = omni_core::js::json_stringify(&serde_json::json!([user_id, client_id]));
    format!("executor:{}", omni_core::digest::sha256_hex(material))
}

/// `^executor:[0-9a-f]{64}$`.
pub fn is_executor_owner(owner: &str) -> bool {
    owner.strip_prefix("executor:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `^Bearer [^\s]{1,8192}$` (case-sensitive scheme, as in TS).
pub fn is_bearer_authorization(value: &str) -> bool {
    value.strip_prefix("Bearer ").is_some_and(|token| {
        !token.is_empty()
            && omni_core::js::utf16_len(token) <= 8192
            && !token.chars().any(char::is_whitespace)
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    user_id: String,
    client_id: String,
    access_token_expires_at: Expiry,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Expiry {
    Number(f64),
    Text(String),
}

/// `createExecutorEventAuthorizer` over Executor's MCP session endpoint.
#[derive(Clone)]
pub struct ExecutorEventAuthorizer {
    http: HttpClient,
    endpoint: Url,
    clock: SharedClock,
    /// The zone `Date.parse` reads zone-less expiries in (the process zone in TS).
    tz: jiff::tz::TimeZone,
}

impl ExecutorEventAuthorizer {
    pub fn new(
        session_url: &str,
        http: HttpClient,
        clock: SharedClock,
    ) -> Result<Self, InvalidSessionUrl> {
        let endpoint = Url::parse(session_url).map_err(|_| InvalidSessionUrl)?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            return Err(InvalidSessionUrl);
        }
        Ok(Self {
            http,
            endpoint,
            clock,
            tz: jiff::tz::TimeZone::UTC,
        })
    }

    /// Reads zone-less expiry strings in `tz` (`TZ`), as `Date.parse` does.
    pub fn with_time_zone(mut self, tz: jiff::tz::TimeZone) -> Self {
        self.tz = tz;
        self
    }

    async fn check(
        &self,
        owner: &str,
        authorization: &str,
    ) -> Result<Option<i64>, ExecutorAuthError> {
        let now = self.clock.now_ms();
        if !is_executor_owner(owner) || !is_bearer_authorization(authorization) {
            return Ok(None);
        }
        let response = self
            .http
            .request(Method::GET, self.endpoint.clone())
            .header("authorization", authorization)
            .redirect(RedirectRule::Error)
            .timeout(REQUEST_TIMEOUT)
            .send_bounded(MAX_SESSION_BYTES)
            .await
            .map_err(|_| ExecutorAuthError)?;
        let status = response.status.as_u16();
        if status == 401 || status == 403 {
            return Ok(None);
        }
        if !response.status.is_success() {
            return Err(ExecutorAuthError);
        }
        let raw: Value = serde_json::from_slice(&response.body).map_err(|_| ExecutorAuthError)?;
        let Ok(session) = serde_json::from_value::<Session>(raw) else {
            return Ok(None);
        };
        let expiry = match &session.access_token_expires_at {
            Expiry::Number(ms) => Some(*ms),
            #[allow(clippy::cast_precision_loss)]
            Expiry::Text(text) => omni_core::js::date_parse(text, &self.tz).map(|ms| ms as f64),
        };
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        Ok(expiry
            .filter(|ms| ms.is_finite() && *ms > now as f64)
            .filter(|_| executor_owner_id(&session.user_id, &session.client_id) == owner)
            .map(|ms| ms as i64))
    }
}

impl EventAuthorizer for ExecutorEventAuthorizer {
    fn authorize<'a>(
        &'a self,
        owner: &'a str,
        authorization: &'a str,
    ) -> BoxFuture<'a, Result<Option<i64>, ExecutorAuthError>> {
        Box::pin(self.check(owner, authorization))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_bearer_patterns() {
        let owner = executor_owner_id("user", "client");
        assert!(is_executor_owner(&owner));
        assert!(!is_executor_owner("executor:short"));
        assert!(!is_executor_owner(&owner.to_uppercase()));
        assert!(is_bearer_authorization("Bearer abc"));
        assert!(!is_bearer_authorization("bearer abc"));
        assert!(!is_bearer_authorization("Bearer a b"));
        assert!(!is_bearer_authorization("Bearer "));
    }
}
