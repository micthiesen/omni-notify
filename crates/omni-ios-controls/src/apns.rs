//! APNs control pushes (`apns.ts`): `POST /3/device/<token>` with push type
//! `controls`, an ES256 provider token cached for 50 minutes, and a 5 s bound.

use std::sync::Mutex;
use std::time::Duration;

use futures::future::BoxFuture;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use omni_api::ios::ApnsEnvironment;
use omni_core::clock::SharedClock;
use omni_http::{HttpClient, Method, SideEffectMode, Url};
use serde::Serialize;

use crate::persistence::IosControlRegistration;

const LOG: &str = "IOSControls";
const PRODUCTION_ORIGIN: &str = "https://api.push.apple.com";
const SANDBOX_ORIGIN: &str = "https://api.sandbox.push.apple.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const TOKEN_LIFETIME_SECS: i64 = 50 * 60;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Signing configuration.
#[derive(Clone)]
pub struct ApnsConfig {
    pub team_id: String,
    pub key_id: String,
    pub bundle_id: String,
    pub private_key_path: String,
}

/// What APNs did with one push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApnsPushResult {
    Sent,
    /// 410 or a token-level rejection: the registration must be removed.
    InvalidToken {
        reason: String,
    },
    Failed {
        status: u16,
        reason: String,
    },
}

/// The push never produced an APNs response.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("APNs {operation} failed: {detail}")]
pub struct ApnsTransportError {
    pub operation: String,
    pub detail: String,
}

impl ApnsTransportError {
    fn new(operation: &str, detail: impl Into<String>) -> Self {
        Self {
            operation: operation.to_owned(),
            detail: detail.into(),
        }
    }
}

/// Sends one "controls changed" push.
pub trait ApnsSender: Send + Sync {
    fn send_control_changed<'a>(
        &'a self,
        registration: &'a IosControlRegistration,
    ) -> BoxFuture<'a, Result<ApnsPushResult, ApnsTransportError>>;
}

/// The request APNs expects for a controls push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApnsControlRequest {
    pub path: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

/// Headers and body of a controls push.
pub fn build_apns_control_request(
    bundle_id: &str,
    push_token: &str,
    provider_token: &str,
) -> ApnsControlRequest {
    let body = r#"{"aps":{"content-changed":true}}"#.to_owned();
    ApnsControlRequest {
        path: format!("/3/device/{push_token}"),
        headers: vec![
            ("authorization", format!("bearer {provider_token}")),
            ("apns-push-type", "controls".to_owned()),
            ("apns-topic", format!("{bundle_id}.push-type.controls")),
            ("apns-priority", "10".to_owned()),
            ("apns-expiration", "0".to_owned()),
            ("content-type", "application/json".to_owned()),
        ],
        body,
    }
}

#[derive(Serialize)]
struct ProviderClaims<'a> {
    iss: &'a str,
    iat: i64,
}

/// An ES256 provider token (`{"alg":"ES256","kid":...}.{"iss":...,"iat":...}`).
pub fn create_apns_provider_token(
    team_id: &str,
    key_id: &str,
    private_key_pem: &str,
    issued_at: i64,
) -> Result<String, ApnsTransportError> {
    let key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
        .map_err(|e| ApnsTransportError::new("provider token creation", e.to_string()))?;
    let mut header = Header::new(Algorithm::ES256);
    header.typ = None;
    header.kid = Some(key_id.to_owned());
    jsonwebtoken::encode(
        &header,
        &ProviderClaims {
            iss: team_id,
            iat: issued_at,
        },
        &key,
    )
    .map_err(|e| ApnsTransportError::new("provider token creation", e.to_string()))
}

/// APNs over the shared HTTP client (HTTP/2 via ALPN).
pub struct ApnsControlClient {
    config: ApnsConfig,
    private_key_pem: String,
    http: HttpClient,
    clock: SharedClock,
    mode: SideEffectMode,
    token: Mutex<Option<(String, i64)>>,
}

impl ApnsControlClient {
    /// Reads the signing key; an unreadable key is a typed construction failure.
    pub async fn create(
        config: ApnsConfig,
        http: HttpClient,
        clock: SharedClock,
        mode: SideEffectMode,
    ) -> Result<Self, ApnsTransportError> {
        let private_key_pem = tokio::fs::read_to_string(&config.private_key_path)
            .await
            .map_err(|e| ApnsTransportError::new("signing key read", e.to_string()))?;
        Ok(Self::with_key(config, private_key_pem, http, clock, mode))
    }

    pub fn with_key(
        config: ApnsConfig,
        private_key_pem: String,
        http: HttpClient,
        clock: SharedClock,
        mode: SideEffectMode,
    ) -> Self {
        Self {
            config,
            private_key_pem,
            http,
            clock,
            mode,
            token: Mutex::new(None),
        }
    }

    fn provider_token(&self) -> Result<String, ApnsTransportError> {
        let issued_at = self.clock.now_ms().div_euclid(1_000);
        let mut cached = self
            .token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((value, at)) = cached.as_ref()
            && issued_at - at < TOKEN_LIFETIME_SECS
        {
            return Ok(value.clone());
        }
        let value = create_apns_provider_token(
            &self.config.team_id,
            &self.config.key_id,
            &self.private_key_pem,
            issued_at,
        )?;
        *cached = Some((value.clone(), issued_at));
        Ok(value)
    }

    async fn send(
        &self,
        registration: &IosControlRegistration,
    ) -> Result<ApnsPushResult, ApnsTransportError> {
        let provider_token = self.provider_token()?;
        let request = build_apns_control_request(
            &self.config.bundle_id,
            &registration.push_token,
            &provider_token,
        );
        if self.mode == SideEffectMode::Record {
            tracing::info!(target: LOG, "Recorded control push for slot {} (side effects recorded)", registration.slot);
            return Ok(ApnsPushResult::Sent);
        }
        let origin = match registration.environment {
            ApnsEnvironment::Sandbox => SANDBOX_ORIGIN,
            ApnsEnvironment::Production => PRODUCTION_ORIGIN,
        };
        let url = Url::parse(&format!("{origin}{}", request.path))
            .map_err(|e| ApnsTransportError::new("request", e.to_string()))?;
        let mut builder = self.http.request(Method::POST, url);
        for (name, value) in &request.headers {
            builder = builder.header(*name, value.as_str());
        }
        let response = builder
            .body(request.body)
            .timeout(REQUEST_TIMEOUT)
            .send_bounded(MAX_RESPONSE_BYTES)
            .await
            .map_err(|e| match e {
                omni_http::HttpError::Timeout => {
                    ApnsTransportError::new("request", "APNs control push timed out")
                }
                other => ApnsTransportError::new("request", other.to_string()),
            })?;
        let status = response.status.as_u16();
        if status == 200 {
            return Ok(ApnsPushResult::Sent);
        }
        let text = String::from_utf8_lossy(&response.body).into_owned();
        let reason = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(str::to_owned))
            .unwrap_or_else(|| {
                if text.is_empty() {
                    format!("HTTP {status}")
                } else {
                    text.clone()
                }
            });
        if status == 410
            || matches!(
                reason.as_str(),
                "BadDeviceToken" | "DeviceTokenNotForTopic" | "Unregistered"
            )
        {
            return Ok(ApnsPushResult::InvalidToken { reason });
        }
        Ok(ApnsPushResult::Failed { status, reason })
    }
}

impl ApnsSender for ApnsControlClient {
    fn send_control_changed<'a>(
        &'a self,
        registration: &'a IosControlRegistration,
    ) -> BoxFuture<'a, Result<ApnsPushResult, ApnsTransportError>> {
        Box::pin(self.send(registration))
    }
}
