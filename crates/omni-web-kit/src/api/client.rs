//! Fetch client for GET, POST, PATCH, PUT and DELETE.
//!
//! GETs ride out container restarts: network failures and 502/503/504 retry
//! with exponential backoff from 500 ms capped at 8 s, at most 7 retries.
//! Mutations never retry. Application errors surface immediately.

use std::fmt;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// `ApiError | ApiNetworkError | ApiDecodeError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiClientError {
    /// A non-2xx response; `message` is the body's `error` or `HTTP <status>: <text>`.
    Api { status: u16, message: String },
    /// The request never produced a response.
    Network { path: String, message: String },
    /// The body was not JSON or did not match the expected shape.
    Decode { path: String, message: String },
}

impl ApiClientError {
    /// HTTP status of an [`ApiClientError::Api`] error.
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiClientError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The user-facing message.
    pub fn message(&self) -> &str {
        match self {
            ApiClientError::Api { message, .. }
            | ApiClientError::Network { message, .. }
            | ApiClientError::Decode { message, .. } => message,
        }
    }
}

impl fmt::Display for ApiClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ApiClientError {}

/// Statuses a GET retries (a restarting container behind the proxy).
pub const RETRYABLE_STATUS: [u16; 3] = [502, 503, 504];
/// Retries after the first attempt.
pub const MAX_GET_RETRIES: usize = 7;

/// Delay before retry number `retry` (0-based): `min(500 ms * 2^retry, 8 s)`.
pub fn get_retry_delay(retry: usize) -> Duration {
    let millis = 500u64.saturating_mul(1u64 << retry.min(20));
    Duration::from_millis(millis.min(8_000))
}

/// What a GET does after one attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GetAttempt {
    /// A response that is final (any status but 502/503/504).
    Done,
    /// Network failure or retryable status with retries left: wait, then retry.
    Retry(Duration),
    /// Retryable failure with no retries left: surface it as is.
    GiveUp,
}

/// The retry decision for attempt `retries_done` (0 for the first request).
/// `status` is `None` for a network failure.
pub fn classify_get_attempt(status: Option<u16>, retries_done: usize) -> GetAttempt {
    let retryable = status.is_none_or(|s| RETRYABLE_STATUS.contains(&s));
    if !retryable {
        GetAttempt::Done
    } else if retries_done < MAX_GET_RETRIES {
        GetAttempt::Retry(get_retry_delay(retries_done))
    } else {
        GetAttempt::GiveUp
    }
}

/// A received response: status, status text and body text.
#[derive(Clone, Debug)]
pub struct RawResponse {
    pub status: u16,
    pub status_text: String,
    pub body: String,
}

impl RawResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The JSON `error` string, else `HTTP <status>: <statusText>`.
    pub fn error_message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|body| body.get("error")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("HTTP {}: {}", self.status, self.status_text))
    }
}

/// Non-2xx → `Api`, invalid JSON or shape → `Decode`.
pub fn decode_response<T: DeserializeOwned>(
    path: &str,
    response: &RawResponse,
) -> Result<T, ApiClientError> {
    if !response.ok() {
        return Err(ApiClientError::Api {
            status: response.status,
            message: response.error_message(),
        });
    }
    let value: Value =
        serde_json::from_str(&response.body).map_err(|_| ApiClientError::Decode {
            path: path.to_owned(),
            message: format!("Invalid JSON response: {path}"),
        })?;
    serde_json::from_value(value).map_err(|_| ApiClientError::Decode {
        path: path.to_owned(),
        message: format!("Response did not match its schema: {path}"),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
    Patch,
    Put,
    Delete,
}

/// One request without retries.
pub(crate) async fn send(
    method: Method,
    path: &str,
    body: Option<String>,
) -> Result<RawResponse, ApiClientError> {
    let network = || ApiClientError::Network {
        path: path.to_owned(),
        message: format!("Request failed: {path}"),
    };
    let builder = match method {
        Method::Get => gloo_net::http::Request::get(path),
        Method::Post => gloo_net::http::Request::post(path),
        Method::Patch => gloo_net::http::Request::patch(path),
        Method::Put => gloo_net::http::Request::put(path),
        Method::Delete => gloo_net::http::Request::delete(path),
    };
    let request = match body {
        Some(body) => builder
            .header("Content-Type", "application/json")
            .body(body)
            .map_err(|_| network())?,
        None => builder.build().map_err(|_| network())?,
    };
    let response = request.send().await.map_err(|_| network())?;
    let status = response.status();
    let status_text = response.status_text();
    let body = response.text().await.unwrap_or_default();
    Ok(RawResponse {
        status,
        status_text,
        body,
    })
}

/// GET with the restart retry policy, returning the final raw response.
pub(crate) async fn get_raw(path: &str) -> Result<RawResponse, ApiClientError> {
    let mut retries = 0;
    loop {
        let result = send(Method::Get, path, None).await;
        let status = result.as_ref().ok().map(|r| r.status);
        match classify_get_attempt(status, retries) {
            GetAttempt::Retry(delay) => {
                crate::task::sleep(delay).await;
                retries += 1;
            }
            GetAttempt::Done | GetAttempt::GiveUp => return result,
        }
    }
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiClientError> {
    let response = get_raw(path).await?;
    decode_response(path, &response)
}

fn encode_body<B: Serialize>(
    path: &str,
    body: Option<&B>,
) -> Result<Option<String>, ApiClientError> {
    body.map(serde_json::to_string)
        .transpose()
        .map_err(|_| ApiClientError::Network {
            path: path.to_owned(),
            message: format!("Request failed: {path}"),
        })
}

/// POSTs a JSON body (never retried).
pub async fn post<T: DeserializeOwned, B: Serialize>(
    path: &str,
    body: Option<&B>,
) -> Result<T, ApiClientError> {
    let body = encode_body(path, body)?;
    let response = send(Method::Post, path, body).await?;
    decode_response(path, &response)
}

/// PATCHes a JSON body (never retried).
pub async fn patch<T: DeserializeOwned, B: Serialize>(
    path: &str,
    body: Option<&B>,
) -> Result<T, ApiClientError> {
    let body = encode_body(path, body)?;
    let response = send(Method::Patch, path, body).await?;
    decode_response(path, &response)
}

/// PUTs a JSON body (never retried).
pub async fn put<T: DeserializeOwned, B: Serialize>(
    path: &str,
    body: Option<&B>,
) -> Result<T, ApiClientError> {
    let body = encode_body(path, body)?;
    let response = send(Method::Put, path, body).await?;
    decode_response(path, &response)
}

/// DELETEs with a JSON body (never retried).
pub async fn delete<T: DeserializeOwned, B: Serialize>(
    path: &str,
    body: Option<&B>,
) -> Result<T, ApiClientError> {
    let body = encode_body(path, body)?;
    let response = send(Method::Delete, path, body).await?;
    decode_response(path, &response)
}

/// Body placeholder for requests without one.
pub const NO_BODY: Option<&()> = None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_retry_schedule_is_exponential_capped_and_bounded() {
        let delays: Vec<u64> = (0..MAX_GET_RETRIES)
            .map(|retry| get_retry_delay(retry).as_millis() as u64)
            .collect();
        assert_eq!(delays, [500, 1000, 2000, 4000, 8000, 8000, 8000]);
        assert_eq!(classify_get_attempt(Some(200), 0), GetAttempt::Done);
        assert_eq!(classify_get_attempt(Some(500), 0), GetAttempt::Done);
        assert_eq!(classify_get_attempt(Some(404), 0), GetAttempt::Done);
        assert_eq!(
            classify_get_attempt(None, 0),
            GetAttempt::Retry(Duration::from_millis(500))
        );
        assert_eq!(
            classify_get_attempt(Some(503), 3),
            GetAttempt::Retry(Duration::from_millis(4000))
        );
        assert_eq!(classify_get_attempt(Some(502), 7), GetAttempt::GiveUp);
        assert_eq!(classify_get_attempt(None, 7), GetAttempt::GiveUp);
    }

    #[test]
    fn decode_maps_errors_to_messages() {
        let error = RawResponse {
            status: 409,
            status_text: "Conflict".into(),
            body: r#"{"error":"Task is already running"}"#.into(),
        };
        assert_eq!(
            decode_response::<Value>("/x", &error),
            Err(ApiClientError::Api {
                status: 409,
                message: "Task is already running".into()
            })
        );
        let html = RawResponse {
            status: 502,
            status_text: "Bad Gateway".into(),
            body: "<html>".into(),
        };
        assert_eq!(
            decode_response::<Value>("/x", &html).map_err(|e| e.to_string()),
            Err("HTTP 502: Bad Gateway".into())
        );
        let invalid = RawResponse {
            status: 200,
            status_text: "OK".into(),
            body: "{".into(),
        };
        assert_eq!(
            decode_response::<Value>("/x", &invalid).map_err(|e| e.to_string()),
            Err("Invalid JSON response: /x".into())
        );
        let wrong = RawResponse {
            status: 200,
            status_text: "OK".into(),
            body: "[]".into(),
        };
        assert_eq!(
            decode_response::<omni_api::tasks::RunNowResponse>("/x", &wrong)
                .map_err(|e| e.to_string()),
            Err("Response did not match its schema: /x".into())
        );
    }
}
