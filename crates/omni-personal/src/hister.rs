//! Hister captured-page archive client.
//!
//! Every response is bounded (8 MiB body, 20 s, decoded field limits) and
//! treated as untrusted. Redirects are refused so the access token is never
//! forwarded, and errors never include the token or a response body. Label
//! writes are verified by reading the document back. Text offsets and
//! truncation count UTF-16 units.

use std::time::Duration;

use omni_core::js::{encode_uri_component, json_stringify, utf16_len, utf16_slice};
use omni_http::{HttpClient, HttpError, Method, SideEffectMode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::reset_alerts::source::optional_non_null;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const PRIOR_SELECTIONS_NOTE: &str = "Previously selected for this query; not constrained by current date filters. May include matching hits omitted from results. Total and cursor describe index matches, not this separate list.";

/// A Hister failure; `reason` never carries upstream content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation}: {reason}")]
pub struct HisterError {
    pub operation: String,
    pub reason: String,
}

impl HisterError {
    pub fn new(operation: &str, reason: &str) -> Self {
        Self {
            operation: operation.to_owned(),
            reason: reason.to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchInput {
    pub query: String,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// `YYYY-MM-DD` (UTC), inclusive.
    pub date_from: Option<String>,
    /// `YYYY-MM-DD` (UTC), inclusive.
    pub date_to: Option<String>,
    pub semantic: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    pub title: String,
    pub title_truncated: bool,
    pub url: String,
    pub snippet: String,
    pub snippet_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub prior_selections: Vec<SearchResult>,
    pub prior_selections_note: String,
    pub total: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageInput {
    pub url: String,
    pub document_id: Option<String>,
    pub offset: Option<u64>,
    pub max_chars: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    pub title: String,
    pub title_truncated: bool,
    pub url: String,
    pub text: String,
    pub total_chars: u64,
    pub offset: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowseInput {
    pub filter: Option<String>,
    /// Unix seconds, inclusive.
    pub date_from: Option<u64>,
    /// Unix seconds, exclusive.
    pub date_to: Option<u64>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    pub title: String,
    pub title_truncated: bool,
    pub url: String,
    pub updated_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_versions: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseResponse {
    pub items: Vec<BrowseItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LabelResponse {
    pub url: String,
    pub label: String,
    pub verified: bool,
}

// --- Upstream wire shapes (decoded strictly, then bounded) ---

#[derive(Deserialize)]
struct WireSearchDocument {
    #[serde(default, deserialize_with = "optional_non_null")]
    id: Option<String>,
    url: String,
    title: String,
    #[serde(default, deserialize_with = "optional_non_null")]
    text: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    snippet: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    label: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    domain: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    updated: Option<f64>,
    #[serde(default, deserialize_with = "optional_non_null")]
    score: Option<f64>,
}

#[derive(Deserialize)]
struct WireSearchResponse {
    total: f64,
    documents: Vec<WireSearchDocument>,
    #[serde(default)]
    history: Option<Vec<WireSearchDocument>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    page_key: Option<String>,
}

#[derive(Deserialize)]
struct WireDocument {
    #[serde(default, deserialize_with = "optional_non_null")]
    id: Option<String>,
    url: String,
    #[serde(default, deserialize_with = "optional_non_null")]
    title: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    text: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    label: Option<String>,
}

#[derive(Deserialize)]
struct WireHistoryDocument {
    #[serde(default, deserialize_with = "optional_non_null")]
    id: Option<String>,
    url: String,
    title: String,
    updated: f64,
    #[serde(default, deserialize_with = "optional_non_null")]
    add_count: Option<f64>,
}

#[derive(Deserialize)]
struct WireHistoryResponse {
    documents: Vec<WireHistoryDocument>,
    #[serde(default, deserialize_with = "optional_non_null")]
    page_key: Option<String>,
}

#[derive(Deserialize)]
struct WireLabelAck {
    ok: bool,
}

fn len_ok(s: &str, max: usize) -> bool {
    utf16_len(s) <= max
}

fn url_ok(s: &str) -> bool {
    !s.is_empty() && len_ok(s, 8_192)
}

fn id_ok(id: Option<&String>) -> bool {
    id.is_none_or(|id| len_ok(id, 8_256))
}

fn non_negative(n: f64) -> bool {
    n.is_finite() && n >= 0.0
}

fn search_document_ok(d: &WireSearchDocument) -> bool {
    id_ok(d.id.as_ref())
        && url_ok(&d.url)
        && d.label.as_deref().is_none_or(|l| len_ok(l, 4_096))
        && d.domain.as_deref().is_none_or(|l| len_ok(l, 1_024))
        && d.updated.is_none_or(non_negative)
        && d.score.is_none_or(f64::is_finite)
}

fn page_key_ok(key: Option<&String>) -> bool {
    key.is_none_or(|key| len_ok(key, 16_384))
}

fn decode<T: for<'de> Deserialize<'de>>(
    value: Value,
    operation: &str,
    valid: impl FnOnce(&T) -> bool,
) -> Result<T, HisterError> {
    let invalid = || HisterError::new(operation, "invalid response");
    let decoded: T = serde_json::from_value(value).map_err(|_| invalid())?;
    if valid(&decoded) {
        Ok(decoded)
    } else {
        Err(invalid())
    }
}

/// `{value, truncated}` at `max` UTF-16 units.
fn bounded(value: &str, max: usize) -> (String, bool) {
    if utf16_len(value) > max {
        (utf16_slice(value, 0, max).into_owned(), true)
    } else {
        (value.to_owned(), false)
    }
}

fn truthy(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

fn search_result(document: WireSearchDocument) -> SearchResult {
    let (snippet, snippet_truncated) = bounded(
        document
            .snippet
            .as_deref()
            .or(document.text.as_deref())
            .unwrap_or(""),
        1_500,
    );
    let (title, title_truncated) = bounded(&document.title, 1_000);
    SearchResult {
        document_id: truthy(document.id),
        title,
        title_truncated,
        url: document.url,
        snippet,
        snippet_truncated,
        label: truthy(document.label),
        domain: truthy(document.domain),
        updated_at: document.updated,
        score: document.score,
    }
}

/// A real calendar date, as Unix seconds at its UTC start (or end).
fn date_to_unix(value: &str, end: bool) -> Option<i64> {
    let date: jiff::civil::Date = value.parse().ok()?;
    if date.to_string() != value {
        return None;
    }
    let seconds = date
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()?
        .timestamp()
        .as_second();
    Some(seconds + if end { 86_399 } else { 0 })
}

/// Records Hister writes skipped in `SideEffectMode::Record`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedLabel {
    pub url: String,
    pub label: String,
}

/// The Hister client.
pub struct HisterService {
    http: HttpClient,
    root: String,
    token: String,
    mode: SideEffectMode,
    recorded: std::sync::Mutex<Vec<RecordedLabel>>,
}

impl HisterService {
    /// Fails unless `base_url` is HTTP(S) without credentials, query or fragment.
    pub fn new(
        base_url: &str,
        access_token: &str,
        http: HttpClient,
        mode: SideEffectMode,
    ) -> Result<Self, HisterError> {
        let origin = Url::parse(base_url).map_err(|_| {
            HisterError::new(
                "configure Hister",
                "Hister URL must use HTTP(S) without credentials, query, or fragment",
            )
        })?;
        if !["http", "https"].contains(&origin.scheme())
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(HisterError::new(
                "configure Hister",
                "Hister URL must use HTTP(S) without credentials, query, or fragment",
            ));
        }
        let root = origin.as_str().trim_end_matches('/').to_owned();
        Ok(Self {
            http,
            root,
            token: access_token.to_owned(),
            mode,
            recorded: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Label writes captured in `SideEffectMode::Record`.
    pub fn recorded_labels(&self) -> Vec<RecordedLabel> {
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn request(
        &self,
        operation: &str,
        path: &str,
        post_body: Option<String>,
    ) -> Result<Value, HisterError> {
        let failed = || HisterError::new(operation, "request failed");
        let url = Url::parse(&format!("{}{path}", self.root)).map_err(|_| failed())?;
        let method = if post_body.is_some() {
            Method::POST
        } else {
            Method::GET
        };
        let mut request = self
            .http
            .request(method, url)
            .header("accept", "application/json")
            .header("x-access-token", self.token.as_str())
            .timeout(REQUEST_TIMEOUT);
        if let Some(body) = post_body {
            request = request
                .header("content-type", "application/json")
                .header("origin", "hister://")
                .body(body);
        }
        // `RedirectRule::Error` (the default): the token is never sent onward.
        let response = match request.send_bounded(MAX_RESPONSE_BYTES).await {
            Ok(response) => response,
            Err(HttpError::Timeout) => {
                return Err(HisterError::new(operation, "request timed out"));
            }
            Err(_) => return Err(failed()),
        };
        if !response.status.is_success() {
            return Err(HisterError::new(
                operation,
                &format!("HTTP {}", response.status.as_u16()),
            ));
        }
        serde_json::from_slice(&response.body).map_err(|_| failed())
    }

    /// Full-text search; prior selections are kept apart from results.
    pub async fn search(&self, input: &SearchInput) -> Result<SearchResponse, HisterError> {
        let invalid = || HisterError::new("search", "invalid request");
        let limit = match input.limit {
            None => 10,
            Some(limit) if (1..=50).contains(&limit) => limit,
            Some(_) => return Err(invalid()),
        };
        let mut query = serde_json::Map::new();
        query.insert("text".into(), Value::String(input.query.clone()));
        query.insert("limit".into(), Value::from(limit));
        query.insert("include_html".into(), Value::Bool(false));
        query.insert("include_text".into(), Value::Bool(true));
        query.insert(
            "semantic_enabled".into(),
            Value::Bool(input.semantic.unwrap_or(false)),
        );
        if let Some(cursor) = input.cursor.as_ref().filter(|c| !c.is_empty()) {
            query.insert("page_key".into(), Value::String(cursor.clone()));
        }
        if let Some(from) = input.date_from.as_deref().filter(|d| !d.is_empty()) {
            query.insert(
                "date_from".into(),
                Value::from(date_to_unix(from, false).ok_or_else(invalid)?),
            );
        }
        if let Some(to) = input.date_to.as_deref().filter(|d| !d.is_empty()) {
            query.insert(
                "date_to".into(),
                Value::from(date_to_unix(to, true).ok_or_else(invalid)?),
            );
        }
        let encoded = encode_uri_component(&json_stringify(&Value::Object(query)));
        let raw = self
            .request(
                "search",
                &format!("/search?format=json&query={encoded}"),
                None,
            )
            .await?;
        let response: WireSearchResponse = decode(raw, "search", |r: &WireSearchResponse| {
            non_negative(r.total)
                && r.documents.len() <= 50
                && r.documents.iter().all(search_document_ok)
                && r.history
                    .as_ref()
                    .is_none_or(|h| h.len() <= 20 && h.iter().all(search_document_ok))
                && page_key_ok(r.page_key.as_ref())
        })?;
        Ok(SearchResponse {
            results: response.documents.into_iter().map(search_result).collect(),
            prior_selections: response
                .history
                .unwrap_or_default()
                .into_iter()
                .map(search_result)
                .collect(),
            prior_selections_note: PRIOR_SELECTIONS_NOTE.to_owned(),
            total: response.total,
            next_cursor: truthy(response.page_key),
        })
    }

    /// Saved page text, sliced at `offset` for at most `maxChars` UTF-16 units.
    pub async fn get_page(&self, input: &PageInput) -> Result<PageResponse, HisterError> {
        let offset = input.offset.unwrap_or(0);
        let max_chars = match input.max_chars {
            None => 20_000,
            Some(n) if (1..=50_000).contains(&n) => n,
            Some(_) => return Err(HisterError::new("get page", "invalid request")),
        };
        let query = {
            let mut params = url::form_urlencoded::Serializer::new(String::new());
            params.append_pair("url", &input.url);
            if let Some(id) = input.document_id.as_deref().filter(|id| !id.is_empty()) {
                params.append_pair("document_id", id);
            }
            params.finish()
        };
        let raw = self
            .request("get page", &format!("/api/document?{query}"), None)
            .await?;
        let document: WireDocument = decode(raw, "get page", |d: &WireDocument| {
            id_ok(d.id.as_ref()) && url_ok(&d.url)
        })?;
        let text = document.text.unwrap_or_default();
        let total = utf16_len(&text) as u64;
        let end = total.min(offset.saturating_add(max_chars));
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let stop = usize::try_from(end).unwrap_or(usize::MAX);
        let slice = if start >= stop {
            String::new()
        } else {
            utf16_slice(&text, start, stop).into_owned()
        };
        let full_title = document.title.unwrap_or_else(|| document.url.clone());
        let (title, title_truncated) = bounded(&full_title, 1_000);
        Ok(PageResponse {
            document_id: truthy(document.id),
            title,
            title_truncated,
            url: document.url,
            text: slice,
            total_chars: total,
            offset,
            next_offset: (end < total).then_some(end),
        })
    }

    /// Recently indexed pages, newest first.
    pub async fn browse(&self, input: &BrowseInput) -> Result<BrowseResponse, HisterError> {
        let query = {
            let mut params = url::form_urlencoded::Serializer::new(String::new());
            if let Some(filter) = input.filter.as_deref().filter(|f| !f.is_empty()) {
                params.append_pair("filter", filter);
            }
            if let Some(from) = input.date_from {
                params.append_pair("date_from", &from.to_string());
            }
            if let Some(to) = input.date_to {
                params.append_pair("date_to", &to.to_string());
            }
            if let Some(cursor) = input.cursor.as_deref().filter(|c| !c.is_empty()) {
                params.append_pair("last", cursor);
            }
            params.finish()
        };
        let raw = self
            .request("browse history", &format!("/api/history?{query}"), None)
            .await?;
        if raw.is_null() {
            return Ok(BrowseResponse {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let response: WireHistoryResponse =
            decode(raw, "browse history", |r: &WireHistoryResponse| {
                r.documents.len() <= 100
                    && page_key_ok(r.page_key.as_ref())
                    && r.documents.iter().all(|d| {
                        id_ok(d.id.as_ref())
                            && url_ok(&d.url)
                            && non_negative(d.updated)
                            && d.add_count.is_none_or(non_negative)
                    })
            })?;
        Ok(BrowseResponse {
            items: response
                .documents
                .into_iter()
                .map(|document| {
                    let (title, title_truncated) = bounded(&document.title, 1_000);
                    BrowseItem {
                        document_id: truthy(document.id),
                        title,
                        title_truncated,
                        url: document.url,
                        updated_at: document.updated,
                        indexed_versions: document.add_count,
                    }
                })
                .collect(),
            next_cursor: truthy(response.page_key),
        })
    }

    /// Replaces (or clears, with `""`) the label of one page, then verifies it
    /// by reading the document back. A mismatch is never retried.
    pub async fn set_label(&self, url: &str, label: &str) -> Result<LabelResponse, HisterError> {
        if self.mode == SideEffectMode::Record {
            self.recorded
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(RecordedLabel {
                    url: url.to_owned(),
                    label: label.to_owned(),
                });
            return Err(HisterError::new(
                "set label",
                "recorded in side-effect record mode; the label was not changed",
            ));
        }
        let body = json_stringify(&json!({"url": url, "label": label}));
        let ack = self.request("set label", "/api/label", Some(body)).await?;
        decode(ack, "set label", |ack: &WireLabelAck| ack.ok)?;
        let raw = self
            .request(
                "verify label",
                &format!("/api/document?url={}", encode_uri_component(url)),
                None,
            )
            .await?;
        let document: WireDocument = decode(raw, "verify label", |d: &WireDocument| {
            id_ok(d.id.as_ref()) && url_ok(&d.url)
        })?;
        if document.url == url && document.label.as_deref().unwrap_or("") == label {
            Ok(LabelResponse {
                url: url.to_owned(),
                label: label.to_owned(),
                verified: true,
            })
        } else {
            Err(HisterError::new("verify label", "label mismatch"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_convert_to_unix_bounds() {
        assert_eq!(date_to_unix("2026-01-01", false), Some(1_767_225_600));
        assert_eq!(date_to_unix("2026-01-02", true), Some(1_767_398_399));
        assert_eq!(date_to_unix("2026-02-30", false), None);
        assert_eq!(date_to_unix("2026-1-01", false), None);
    }
}
