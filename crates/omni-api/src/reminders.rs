//! The public Reminders administration status
//! (`GET /api/reminders/status`, `POST /api/reminders/auth/{start,code,verify}`).
//!
//! These payloads carry no account data: only a bounded phase, reason, challenge
//! handle and a diagnostic stage/category with an optional Apple HTTP status.

use serde::{Deserialize, Serialize};

/// The connection phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "authenticated")]
    Authenticated,
    #[serde(rename = "authentication-needed")]
    AuthenticationNeeded,
    #[serde(rename = "transient-outage")]
    TransientOutage,
    #[serde(rename = "rate-limited")]
    RateLimited,
    #[serde(rename = "unsupported-protocol")]
    UnsupportedProtocol,
    #[serde(rename = "awaiting-device-approval")]
    AwaitingDeviceApproval,
    #[serde(rename = "terms-required")]
    TermsRequired,
}

/// Why the phase applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Reason {
    #[serde(rename = "mfa")]
    Mfa,
    #[serde(rename = "pcs")]
    Pcs,
    #[serde(rename = "terms")]
    Terms,
    #[serde(rename = "credentials")]
    Credentials,
    #[serde(rename = "session-expired")]
    SessionExpired,
    #[serde(rename = "configuration")]
    Configuration,
    #[serde(rename = "protocol")]
    Protocol,
}

/// The failed step of the last Apple exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiagnosticStage {
    #[serde(rename = "sign-in-init")]
    SignInInit,
    #[serde(rename = "sign-in-proof")]
    SignInProof,
    #[serde(rename = "sign-in-complete")]
    SignInComplete,
    #[serde(rename = "account-session")]
    AccountSession,
    #[serde(rename = "second-factor")]
    SecondFactor,
    #[serde(rename = "second-factor-options")]
    SecondFactorOptions,
    #[serde(rename = "device-notification")]
    DeviceNotification,
    #[serde(rename = "code-verification")]
    CodeVerification,
    #[serde(rename = "protected-data-access")]
    ProtectedDataAccess,
    #[serde(rename = "apple-request")]
    AppleRequest,
    #[serde(rename = "private-storage")]
    PrivateStorage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiagnosticCategory {
    #[serde(rename = "apple-response")]
    AppleResponse,
    #[serde(rename = "transport")]
    Transport,
    #[serde(rename = "protocol")]
    Protocol,
    #[serde(rename = "storage")]
    Storage,
    #[serde(rename = "authentication")]
    Authentication,
}

/// A Reminders diagnostic; `httpStatus` only within 100..=599.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub stage: DiagnosticStage,
    pub category: DiagnosticCategory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicStatus {
    pub enabled: bool,
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge_expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<Diagnostic>,
}

impl PublicStatus {
    /// `{enabled, phase}` without reason, challenge or diagnostic.
    pub fn new(enabled: bool, phase: Phase) -> Self {
        Self {
            enabled,
            phase,
            reason: None,
            challenge_id: None,
            challenge_expires_at: None,
            diagnostic: None,
        }
    }

    pub fn with_reason(mut self, reason: Reason) -> Self {
        self.reason = Some(reason);
        self
    }
}

/// `{status}` (every successful route response).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub status: PublicStatus,
}

/// `{error, status?}` (failed start/verify include the current status).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusErrorResponse {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PublicStatus>,
}

/// `POST /api/reminders/auth/code` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRequest {
    pub challenge_id: String,
    pub code: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_the_wire_shape() {
        let raw = r#"{"enabled":true,"phase":"transient-outage","reason":"pcs","challengeId":"c","challengeExpiresAt":1800000000000,"diagnostic":{"stage":"sign-in-init","category":"apple-response","httpStatus":503}}"#;
        let status: PublicStatus = serde_json::from_str(raw).unwrap();
        assert_eq!(serde_json::to_string(&status).unwrap(), raw);
        let minimal = serde_json::to_string(&StatusResponse {
            status: PublicStatus::new(false, Phase::Disabled).with_reason(Reason::Configuration),
        })
        .unwrap();
        assert_eq!(
            minimal,
            r#"{"status":{"enabled":false,"phase":"disabled","reason":"configuration"}}"#
        );
    }
}
