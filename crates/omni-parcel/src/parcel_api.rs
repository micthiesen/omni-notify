//! Parcel `add-delivery` submission.
//! In `SideEffectMode::Record` nothing is sent: the payload is recorded and
//! reported as accepted.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_core::js::{json_stringify, json_stringify_pretty2, to_iso_string};
use omni_http::{HttpClient, HttpError, Method, SideEffectMode, Url};
use serde_json::json;

use crate::log_file::{LogFile, code_block};

const LOG: &str = "Main:ParcelTracker";
pub const ADD_DELIVERY_URL: &str = "https://api.parcel.app/external/add-delivery/";
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_LIMIT: usize = 1024 * 1024;

/// `SubmitResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitResult {
    Success,
    /// A 4xx answer.
    Rejected {
        status: u16,
    },
    /// Network failure or any other status: transient.
    Error,
}

/// A 4xx rejection other than auth or rate limit
/// plausibly means the wrong carrier was picked.
pub fn should_try_next_candidate(result: SubmitResult) -> bool {
    match result {
        SubmitResult::Rejected { status } => !matches!(status, 401 | 403 | 429),
        SubmitResult::Success | SubmitResult::Error => false,
    }
}

/// One submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitParams {
    pub tracking_number: String,
    pub carrier_code: String,
    pub description: String,
}

/// The Parcel call (a test seam).
pub trait ParcelSubmitter: Send + Sync {
    fn submit<'a>(
        &'a self,
        params: &'a SubmitParams,
        rejection_log: Option<&'a LogFile>,
    ) -> BoxFuture<'a, SubmitResult>;
}

/// The HTTP submitter.
pub struct ParcelApi {
    http: HttpClient,
    api_key: String,
    mode: SideEffectMode,
    url: Url,
    clock: omni_core::clock::SharedClock,
    recorded: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl ParcelApi {
    pub fn new(
        http: HttpClient,
        api_key: String,
        mode: SideEffectMode,
        clock: omni_core::clock::SharedClock,
    ) -> Result<Self, HttpError> {
        let url = Url::parse(ADD_DELIVERY_URL).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
        Ok(Self {
            http,
            api_key,
            mode,
            url,
            clock,
            recorded: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Payloads recorded instead of sent (`SideEffectMode::Record`).
    pub fn recorded(&self) -> Vec<serde_json::Value> {
        self.recorded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    async fn send(&self, params: &SubmitParams, rejection_log: Option<&LogFile>) -> SubmitResult {
        let payload = json!({
            "tracking_number": params.tracking_number,
            "carrier_code": params.carrier_code,
            "description": params.description,
            "send_push_confirmation": true,
        });
        tracing::info!(target: LOG, "Submitting delivery: {}", json_stringify(&payload));
        if self.mode == SideEffectMode::Record {
            self.recorded
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(payload);
            tracing::info!(
                target: LOG,
                "Recorded delivery (side effects recorded): {} ({})",
                params.tracking_number,
                params.carrier_code
            );
            return SubmitResult::Success;
        }
        let response = self
            .http
            .request(Method::POST, self.url.clone())
            .header("api-key", self.api_key.as_str())
            .json(&payload)
            .timeout(SUBMIT_TIMEOUT)
            .send_bounded(RESPONSE_LIMIT)
            .await;
        let (status, body, message) = match response {
            Ok(response) if response.status.is_success() => {
                tracing::info!(
                    target: LOG,
                    "Submitted delivery: {} ({}) → {}",
                    params.tracking_number,
                    params.carrier_code,
                    response.status.as_u16()
                );
                return SubmitResult::Success;
            }
            Ok(response) => {
                let status = response.status.as_u16();
                let body = String::from_utf8_lossy(&response.body).into_owned();
                (Some(status), body, format!("Response code {status}"))
            }
            Err(error) => (None, "no response body".to_owned(), error.to_string()),
        };
        tracing::error!(
            target: LOG,
            detail = %format!("Parcel submission failed: {message}\nResponse: {body}"),
            "Failed to submit delivery {}",
            params.tracking_number
        );
        match status {
            Some(status) if (400..500).contains(&status) => {
                if let Some(log) = rejection_log {
                    log.section(
                        &format!(
                            "Rejected: {} ({status}) — {}",
                            params.tracking_number,
                            to_iso_string(self.clock.now_ms())
                        ),
                        &format!(
                            "**Request:**\n{}\n\n**Response:**\n{}",
                            code_block(&json_stringify_pretty2(&payload), Some("json")),
                            code_block(&body, None)
                        ),
                    )
                    .await;
                }
                SubmitResult::Rejected { status }
            }
            _ => SubmitResult::Error,
        }
    }
}

impl ParcelSubmitter for ParcelApi {
    fn submit<'a>(
        &'a self,
        params: &'a SubmitParams,
        rejection_log: Option<&'a LogFile>,
    ) -> BoxFuture<'a, SubmitResult> {
        Box::pin(self.send(params, rejection_log))
    }
}

#[cfg(test)]
mod parcel_api_spec {
    use super::*;

    #[test]
    fn returns_true_for_a_400_rejection_likely_wrong_carrier() {
        assert!(should_try_next_candidate(SubmitResult::Rejected {
            status: 400
        }));
    }

    #[test]
    fn returns_true_for_other_4xx_rejections_like_404_and_422() {
        assert!(should_try_next_candidate(SubmitResult::Rejected {
            status: 404
        }));
        assert!(should_try_next_candidate(SubmitResult::Rejected {
            status: 422
        }));
    }

    #[test]
    fn returns_false_for_auth_rejections_not_carrier_related() {
        assert!(!should_try_next_candidate(SubmitResult::Rejected {
            status: 401
        }));
        assert!(!should_try_next_candidate(SubmitResult::Rejected {
            status: 403
        }));
    }

    #[test]
    fn returns_false_for_rate_limit_rejections() {
        assert!(!should_try_next_candidate(SubmitResult::Rejected {
            status: 429
        }));
    }

    #[test]
    fn returns_false_for_transient_errors_network_5xx() {
        assert!(!should_try_next_candidate(SubmitResult::Error));
    }

    #[test]
    fn returns_false_for_success() {
        assert!(!should_try_next_candidate(SubmitResult::Success));
    }
}
