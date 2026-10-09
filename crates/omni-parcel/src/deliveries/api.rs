//! The Parcel `deliveries` read. Only the scheduled `ParcelDeliveries` task
//! calls it, after reserving a slot in the durable read budget.

use std::time::Duration;

use omni_http::{HttpClient, HttpError, Method, StatusCode, Url};
use serde::Deserialize;

pub const DELIVERIES_URL: &str = "https://api.parcel.app/external/deliveries/?filter_mode=recent";
const READ_TIMEOUT: Duration = Duration::from_secs(15);
pub const RESPONSE_LIMIT: usize = 1024 * 1024;
/// The response body excerpt kept in errors.
const BODY_EXCERPT: usize = 300;

/// One delivery as Parcel returns it. Every field Parcel documents beyond
/// the tracking number and status is optional.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct RawDelivery {
    pub tracking_number: String,
    #[serde(default)]
    pub carrier_code: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub status_code: i64,
    #[serde(default)]
    pub extra_information: Option<String>,
    #[serde(default)]
    pub date_expected: Option<String>,
    #[serde(default)]
    pub date_expected_end: Option<String>,
    #[serde(default)]
    pub events: Vec<RawEvent>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct RawEvent {
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub additional: Option<String>,
}

#[derive(Deserialize)]
struct RawResponse {
    success: bool,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    deliveries: Vec<RawDelivery>,
}

#[derive(Debug, thiserror::Error)]
pub enum DeliveriesError {
    /// HTTP 429; `retry_after_ms` from `Retry-After` seconds when present.
    #[error("Parcel rate limit reached (HTTP 429)")]
    RateLimited { retry_after_ms: Option<i64> },
    #[error("Parcel deliveries read failed: HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("Parcel deliveries read failed: {0}")]
    Http(#[from] HttpError),
    #[error("Parcel deliveries response was not understood: {0}")]
    Decode(String),
    #[error("Parcel refused the deliveries read: {0}")]
    Unsuccessful(String),
}

impl DeliveriesError {
    /// Network failures and 5xx answers; the next scheduled read retries.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http(error) => error.is_transient(),
            Self::Status { status, .. } => *status >= 500,
            Self::RateLimited { .. } | Self::Decode(_) | Self::Unsuccessful(_) => false,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::RateLimited { .. } => Some(429),
            Self::Status { status, .. } => Some(*status),
            Self::Http(HttpError::Status { status, .. }) => Some(*status),
            _ => None,
        }
    }
}

/// Decodes a 2xx deliveries body.
pub fn decode_deliveries(body: &[u8]) -> Result<Vec<RawDelivery>, DeliveriesError> {
    let response: RawResponse =
        serde_json::from_slice(body).map_err(|e| DeliveriesError::Decode(e.to_string()))?;
    if !response.success {
        return Err(DeliveriesError::Unsuccessful(
            response
                .error_message
                .unwrap_or_else(|| "success: false".to_owned()),
        ));
    }
    Ok(response.deliveries)
}

/// The `GET deliveries` client.
#[derive(Clone)]
pub struct DeliveriesClient {
    http: HttpClient,
    api_key: String,
    url: Url,
}

impl DeliveriesClient {
    pub fn new(http: HttpClient, api_key: String) -> Result<Self, HttpError> {
        let url = Url::parse(DELIVERIES_URL).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
        Ok(Self { http, api_key, url })
    }

    /// Exactly one request; never retries.
    pub async fn fetch(&self) -> Result<Vec<RawDelivery>, DeliveriesError> {
        let response = self
            .http
            .request(Method::GET, self.url.clone())
            .header("api-key", self.api_key.as_str())
            .timeout(READ_TIMEOUT)
            .send_bounded(RESPONSE_LIMIT)
            .await?;
        if response.status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after_ms = response
                .headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map(|secs| secs.saturating_mul(1000));
            return Err(DeliveriesError::RateLimited { retry_after_ms });
        }
        if !response.status.is_success() {
            return Err(DeliveriesError::Status {
                status: response.status.as_u16(),
                body: String::from_utf8_lossy(&response.body)
                    .chars()
                    .take(BODY_EXCERPT)
                    .collect(),
            });
        }
        decode_deliveries(&response.body)
    }
}
