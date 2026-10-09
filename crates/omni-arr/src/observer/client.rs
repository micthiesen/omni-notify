//! Observer (Overseerr) issue API client (`src/observer/client.ts`).

use std::time::Duration;

use omni_http::{HttpClient, Method, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::js_opt::JsonOpt;
use crate::json_api::{ApiFailure, JsonApi};
use crate::side_effects::SideEffects;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 20;
const MAX_RECORDS: usize = PAGE_SIZE * MAX_PAGES;

/// `ObserverClientError`: `"<operation> failed: <detail>"`; never contains the API key.
#[derive(Debug, thiserror::Error)]
#[error("{operation} failed: {cause}")]
pub struct ObserverClientError {
    pub operation: String,
    #[source]
    pub cause: ApiFailure,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObserverUser {
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub id: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub username: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub display_name: JsonOpt<String>,
}

/// `MediaInfoSchema`, absent and `null` kept apart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub id: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub tmdb_id: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub tvdb_id: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub status: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub media_type: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub title: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub external_service_id: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub external_service_id4k: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub external_service_slug: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub external_service_slug4k: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub service_id: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub service_id4k: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub service_url: JsonOpt<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueComment {
    pub id: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub user: JsonOpt<ObserverUser>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub created_at: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub updated_at: JsonOpt<String>,
}

/// `ObserverIssue` (`IssueSchema`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObserverIssue {
    pub id: i64,
    pub issue_type: i64,
    /// 1 open, 2 resolved.
    pub status: i64,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub problem_season: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub problem_episode: JsonOpt<i64>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub created_at: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub updated_at: JsonOpt<String>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub media: JsonOpt<MediaInfo>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub created_by: JsonOpt<ObserverUser>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub modified_by: JsonOpt<ObserverUser>,
    #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
    pub comments: JsonOpt<Vec<IssueComment>>,
}

impl ObserverIssue {
    /// `issue.media?.mediaType`.
    pub fn media_type(&self) -> Option<&str> {
        self.media
            .as_option()
            .and_then(|media| media.media_type.as_option())
            .map(String::as_str)
    }

    /// `issue.comments ?? []`.
    pub fn comment_list(&self) -> &[IssueComment] {
        self.comments.as_option().map_or(&[], Vec::as_slice)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct PageInfo {
    page: f64,
    pages: f64,
    results: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuePage {
    page_info: PageInfo,
    results: Vec<ObserverIssue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueFilter {
    All,
    Open,
    Resolved,
}

impl IssueFilter {
    fn as_str(self) -> &'static str {
        match self {
            IssueFilter::All => "all",
            IssueFilter::Open => "open",
            IssueFilter::Resolved => "resolved",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ListIssuesOptions {
    pub filter: IssueFilter,
    pub max_pages: usize,
    pub max_records: usize,
}

impl Default for ListIssuesOptions {
    fn default() -> Self {
        Self {
            filter: IssueFilter::Open,
            max_pages: MAX_PAGES,
            max_records: MAX_RECORDS,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ObserverClientConfig {
    pub url: String,
    pub api_key: String,
    pub http: HttpClient,
    pub side_effects: SideEffects,
}

/// Overseerr `/api/v1` issue operations.
#[derive(Clone, Debug)]
pub struct ObserverClient {
    base_url: String,
    api: JsonApi,
}

impl ObserverClient {
    pub fn new(config: ObserverClientConfig) -> Self {
        Self {
            base_url: config
                .url
                .strip_suffix('/')
                .unwrap_or(&config.url)
                .to_owned(),
            api: JsonApi::new(
                config.http,
                config.api_key,
                config.side_effects,
                MAX_RESPONSE_BYTES,
                REQUEST_TIMEOUT,
            ),
        }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, ObserverClientError> {
        let operation = format!("Observer {method} {path}");
        let fail = |cause| ObserverClientError {
            operation: operation.clone(),
            cause,
        };
        let url = Url::parse(&format!("{}/api/v1{path}", self.base_url))
            .map_err(|e| fail(ApiFailure::Url(e.to_string())))?;
        self.api.json(method, url, body).await.map_err(fail)
    }

    pub async fn get_issue(&self, issue_id: i64) -> Result<ObserverIssue, ObserverClientError> {
        self.request(Method::GET, &format!("/issue/{issue_id}"), None)
            .await
    }

    /// Bounded paging: at most 20 pages of 100 and 2,000 records.
    pub async fn list_issues(
        &self,
        options: ListIssuesOptions,
    ) -> Result<Vec<ObserverIssue>, ObserverClientError> {
        let max_pages = options.max_pages.clamp(1, MAX_PAGES);
        let max_records = options.max_records.clamp(1, MAX_RECORDS);
        let mut issues: Vec<ObserverIssue> = Vec::new();
        let mut page = 0;
        while page < max_pages && issues.len() < max_records {
            let take = PAGE_SIZE.min(max_records - issues.len());
            let response: IssuePage = self
                .request(
                    Method::GET,
                    &format!(
                        "/issue?take={take}&skip={}&filter={}",
                        issues.len(),
                        options.filter.as_str()
                    ),
                    None,
                )
                .await?;
            let room = max_records - issues.len();
            let received = response.results.len();
            issues.extend(response.results.into_iter().take(room));
            if response.page_info.page >= response.page_info.pages || received == 0 {
                break;
            }
            page += 1;
        }
        Ok(issues)
    }

    pub async fn add_comment(
        &self,
        issue_id: i64,
        message: &str,
    ) -> Result<ObserverIssue, ObserverClientError> {
        self.request(
            Method::POST,
            &format!("/issue/{issue_id}/comment"),
            Some(json!({ "message": message })),
        )
        .await
    }

    pub async fn resolve_issue(&self, issue_id: i64) -> Result<ObserverIssue, ObserverClientError> {
        self.request(Method::POST, &format!("/issue/{issue_id}/resolved"), None)
            .await
    }

    pub async fn reopen_issue(&self, issue_id: i64) -> Result<ObserverIssue, ObserverClientError> {
        self.request(Method::POST, &format!("/issue/{issue_id}/open"), None)
            .await
    }
}
