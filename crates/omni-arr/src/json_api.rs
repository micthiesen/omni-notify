//! Bounded, authenticated JSON requests shared by the Arr and Observer clients.

use std::time::Duration;

use omni_http::{HttpClient, HttpError, Method, Url};
use serde::de::DeserializeOwned;

use crate::side_effects::{RecordedMutation, SideEffects};

/// Why a request failed. Messages never include the API key.
#[derive(Debug, thiserror::Error)]
pub enum ApiFailure {
    #[error("HTTP {0}")]
    Status(u16),
    #[error("{0}")]
    Http(#[source] HttpError),
    #[error("{0}")]
    Json(#[source] serde_json::Error),
    #[error("mutation recorded, not sent (side-effect record mode)")]
    Recorded,
    #[error("invalid URL: {0}")]
    Url(String),
}

/// One service's API: key header, response cap and per-request timeout.
#[derive(Clone, Debug)]
pub(crate) struct JsonApi {
    http: HttpClient,
    api_key: String,
    side_effects: SideEffects,
    max_bytes: usize,
    timeout: Duration,
}

impl JsonApi {
    pub(crate) fn new(
        http: HttpClient,
        api_key: String,
        side_effects: SideEffects,
        max_bytes: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            http,
            api_key,
            side_effects,
            max_bytes,
            timeout,
        }
    }

    /// Sends a request with a JSON body (when given) and decodes the JSON response.
    pub(crate) async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<serde_json::Value>,
    ) -> Result<T, ApiFailure> {
        let response = self.send(method, url, body, true).await?;
        serde_json::from_slice(&response).map_err(ApiFailure::Json)
    }

    /// Sends a request whose response body is ignored.
    pub(crate) async fn void(&self, method: Method, url: Url) -> Result<(), ApiFailure> {
        self.send(method, url, None, false).await.map(|_| ())
    }

    async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<serde_json::Value>,
        json_content: bool,
    ) -> Result<Vec<u8>, ApiFailure> {
        if method != Method::GET
            && self.side_effects.intercept(RecordedMutation {
                method: method.to_string(),
                url: url.to_string(),
                body: body.clone(),
            })
        {
            return Err(ApiFailure::Recorded);
        }
        let mut request = self
            .http
            .request(method, url)
            .header("Accept", "application/json")
            .header("X-Api-Key", self.api_key.as_str())
            .timeout(self.timeout);
        if json_content {
            request = request.header("Content-Type", "application/json");
        }
        if let Some(body) = body {
            let text = serde_json::to_vec(&body).map_err(ApiFailure::Json)?;
            request = request.body(text);
        }
        let response = request
            .send_bounded(self.max_bytes)
            .await
            .map_err(ApiFailure::Http)?;
        if !response.status.is_success() {
            return Err(ApiFailure::Status(response.status.as_u16()));
        }
        Ok(response.body.to_vec())
    }
}
