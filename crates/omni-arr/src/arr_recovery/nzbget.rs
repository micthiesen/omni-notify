//! Read-only NZBGet corroboration of Arr's no-files rejection
//! (`src/arr-recovery/nzbget.ts`).

use std::sync::LazyLock;
use std::time::Duration;

use omni_http::{HttpClient, Method, Url};
use regex::Regex;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::types::{ArrCause, ArrKind, ArrRecoveryError, ArrResult};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Parameter {
    name: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Group {
    #[serde(rename = "NZBID")]
    #[allow(dead_code)]
    nzb_id: f64,
    parameters: Vec<Parameter>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HistoryItem {
    #[serde(rename = "NZBID")]
    #[allow(dead_code)]
    nzb_id: f64,
    status: String,
    category: String,
    dest_dir: String,
    final_dir: String,
    parameters: Vec<Parameter>,
}

#[derive(Deserialize)]
struct RpcResult<T> {
    result: Vec<T>,
}

static TERMINAL_STATUS: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^(?:WARNING/HEALTH|FAILURE/(?:HEALTH|PAR|UNPACK|PASSWORD))$").ok()
});

fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUS
        .as_ref()
        .is_some_and(|re| re.is_match(status))
}

fn matches(parameters: &[Parameter], download_id: &str) -> bool {
    parameters
        .iter()
        .any(|p| p.name == "drone" && p.value == download_id)
}

/// A trusted local NZBGet JSON-RPC endpoint (`NZBGET_URL`); only read methods are called.
#[derive(Clone, Debug)]
pub struct NzbGetClient {
    http: HttpClient,
    url: String,
}

impl NzbGetClient {
    pub fn new(http: HttpClient, url: impl Into<String>) -> Self {
        Self {
            http,
            url: url.into(),
        }
    }

    async fn rpc<T: DeserializeOwned>(&self, method: &str, params: Value) -> ArrResult<Vec<T>> {
        let operation = format!("NZBGet {method}");
        let fail = |cause: &str| ArrRecoveryError::message(operation.as_str(), cause);
        let url = Url::parse(&format!("{}/", self.url.trim_end_matches('/')))
            .and_then(|base| base.join("jsonrpc"))
            .map_err(|_| fail("Request failed"))?;
        let body = serde_json::to_vec(&json!({ "method": method, "params": params, "id": 1 }))
            .map_err(|_| fail("Request failed"))?;
        let response = self
            .http
            .request(Method::POST, url)
            .header("Content-Type", "application/json")
            .body(body)
            .send_bounded(MAX_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                omni_http::HttpError::TooLarge { .. } => fail("Invalid response"),
                _ => fail("Request failed"),
            })?;
        if !response.status.is_success() {
            return Err(fail(&format!("HTTP {}", response.status.as_u16())));
        }
        let raw: Value =
            serde_json::from_slice(&response.body).map_err(|_| fail("Invalid JSON"))?;
        serde_json::from_value::<RpcResult<T>>(raw)
            .map(|decoded| decoded.result)
            .map_err(|_| fail("Unexpected response schema"))
    }

    async fn health(
        &self,
        kind: ArrKind,
        download_id: &str,
        output_path: &str,
    ) -> ArrResult<Option<String>> {
        let active: Vec<Group> = self.rpc("listgroups", json!([])).await?;
        if active.iter().any(|g| matches(&g.parameters, download_id)) {
            return Ok(None);
        }
        let history: Vec<HistoryItem> = self.rpc("history", json!([false])).await?;
        let mut items = history
            .into_iter()
            .filter(|item| matches(&item.parameters, download_id));
        let (Some(item), None) = (items.next(), items.next()) else {
            return Ok(None);
        };
        let dir = if item.final_dir.is_empty() {
            &item.dest_dir
        } else {
            &item.final_dir
        };
        if item.category != kind.as_str() || dir != output_path {
            return Ok(None);
        }
        if !is_terminal(&item.status) {
            return Ok(None);
        }
        // Fetch active work again: a retry may have started during the history read.
        let active: Vec<Group> = self.rpc("listgroups", json!([])).await?;
        if active.iter().any(|g| matches(&g.parameters, download_id)) {
            return Ok(None);
        }
        Ok(Some(item.status))
    }

    /// `downloadHealth`: the terminal NZBGet status of exactly this Arr download
    /// (category and directory must match), or `None` when it cannot corroborate.
    pub async fn download_health(
        &self,
        kind: ArrKind,
        download_id: &str,
        output_path: &str,
    ) -> ArrResult<Option<String>> {
        const OPERATION: &str = "confirm terminal download failure";
        match tokio::time::timeout(TIMEOUT, self.health(kind, download_id, output_path)).await {
            Ok(result) => result.map_err(|inner| ArrRecoveryError::wrap(OPERATION, inner)),
            Err(_) => Err(ArrRecoveryError::new(
                OPERATION,
                ArrCause::Timeout(TIMEOUT.as_secs()),
            )),
        }
    }
}
