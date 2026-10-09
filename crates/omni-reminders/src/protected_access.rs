//! Service-independent iCloud PCS (protected data) access.
//!
//! Adapted from MIT-licensed pyicloud PR 317 (b5f2e2a7f9e5cd5be7e009626c4ae021d8b2bb34,
//! `pyicloud/base.py`); see docs/licenses/pyicloud-MIT.txt. The caller owns
//! authenticated requests, cookie persistence, and account locking. Only an explicit
//! user action starts this workflow; it never approves device prompts.

use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Value, json};

/// The three PCS steps (`ICloudPcsError.operation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcsOperation {
    State,
    Consent,
    Cookies,
}

impl PcsOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::State => "PCS state",
            Self::Consent => "PCS consent",
            Self::Cookies => "PCS cookies",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcsEndpoint {
    RequestWebAccessState,
    EnableDeviceConsentForPcs,
    RequestPcs,
}

impl PcsEndpoint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequestWebAccessState => "requestWebAccessState",
            Self::EnableDeviceConsentForPcs => "enableDeviceConsentForPCS",
            Self::RequestPcs => "requestPCS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcsFailureReason {
    InvalidAppName,
    Http,
    InvalidResponse,
    ConsentNotSent,
    UnknownState,
}

/// A PCS failure; never carries Apple's response body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{} failed", .operation.as_str())]
pub struct ICloudPcsError {
    pub operation: PcsOperation,
    pub status: Option<u16>,
    pub reason: PcsFailureReason,
}

/// A request failure from the caller, or a PCS protocol failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcsError<E> {
    Request(E),
    Pcs(ICloudPcsError),
}

/// The outcome of an explicit access request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedAccess {
    NotRequired,
    ConsentRequired,
    Ready,
}

/// The status and decoded JSON body of one authenticated setup request.
#[derive(Clone, Debug, PartialEq)]
pub struct PcsResponse {
    pub status: u16,
    pub data: Value,
}

/// The caller's authenticated, cookie-persisting request adapter.
pub trait PcsRequester<E>: Send + Sync {
    fn request(
        &self,
        operation: PcsOperation,
        endpoint: PcsEndpoint,
        body: Option<Value>,
    ) -> BoxFuture<'_, Result<PcsResponse, E>>;
}

fn valid_app_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

async fn decoded<E, T>(
    requester: &dyn PcsRequester<E>,
    operation: PcsOperation,
    endpoint: PcsEndpoint,
    body: Option<Value>,
    decode: impl Fn(&Value) -> Option<T>,
) -> Result<T, PcsError<E>> {
    let response = requester
        .request(operation, endpoint, body)
        .await
        .map_err(PcsError::Request)?;
    if response.status != 200 {
        return Err(PcsError::Pcs(ICloudPcsError {
            operation,
            status: Some(response.status),
            reason: PcsFailureReason::Http,
        }));
    }
    decode(&response.data).ok_or(PcsError::Pcs(ICloudPcsError {
        operation,
        status: Some(200),
        reason: PcsFailureReason::InvalidResponse,
    }))
}

/// An optional struct field: absent, or a boolean.
fn optional_bool(object: &serde_json::Map<String, Value>, key: &str) -> Option<Option<bool>> {
    match object.get(key) {
        None => Some(None),
        Some(Value::Bool(b)) => Some(Some(*b)),
        Some(_) => None,
    }
}

/// Requests protected-data access for `app_name`, polling at five-second
/// intervals with `derivedFromUserAction: false` after the first request, up to
/// ten requests. Pending approval returns `ConsentRequired` without claiming access.
pub async fn request_protected_access<E>(
    requester: &dyn PcsRequester<E>,
    app_name: &str,
) -> Result<ProtectedAccess, PcsError<E>> {
    if !valid_app_name(app_name) {
        return Err(PcsError::Pcs(ICloudPcsError {
            operation: PcsOperation::State,
            status: None,
            reason: PcsFailureReason::InvalidAppName,
        }));
    }
    let (disabled, consented) = decoded(
        requester,
        PcsOperation::State,
        PcsEndpoint::RequestWebAccessState,
        None,
        |data| {
            let object = data.as_object()?;
            let disabled = object.get("isICDRSDisabled")?.as_bool()?;
            let consented = optional_bool(object, "isDeviceConsentedForPCS")?;
            Some((disabled, consented))
        },
    )
    .await?;
    if !disabled {
        return Ok(ProtectedAccess::NotRequired);
    }
    if consented != Some(true) {
        let sent = decoded(
            requester,
            PcsOperation::Consent,
            PcsEndpoint::EnableDeviceConsentForPcs,
            None,
            |data| {
                data.as_object()?
                    .get("isDeviceConsentNotificationSent")?
                    .as_bool()
            },
        )
        .await?;
        if !sent {
            return Err(PcsError::Pcs(ICloudPcsError {
                operation: PcsOperation::Consent,
                status: Some(200),
                reason: PcsFailureReason::ConsentNotSent,
            }));
        }
        return Ok(ProtectedAccess::ConsentRequired);
    }
    for attempt in 0..10 {
        let (status, message) = decoded(
            requester,
            PcsOperation::Cookies,
            PcsEndpoint::RequestPcs,
            Some(json!({"appName": app_name, "derivedFromUserAction": attempt == 0})),
            |data| {
                let object = data.as_object()?;
                let status = object.get("status")?.as_str()?.to_owned();
                let message = match object.get("message") {
                    None => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(_) => return None,
                };
                Some((status, message))
            },
        )
        .await?;
        if status == "success" {
            return Ok(ProtectedAccess::Ready);
        }
        if !matches!(
            message.as_deref(),
            Some(
                "Requested the device to upload cookies." | "Cookies not available yet on server."
            )
        ) {
            return Err(PcsError::Pcs(ICloudPcsError {
                operation: PcsOperation::Cookies,
                status: Some(200),
                reason: PcsFailureReason::UnknownState,
            }));
        }
        if attempt < 9 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
    Ok(ProtectedAccess::ConsentRequired)
}
