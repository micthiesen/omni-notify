//! Low-level client for the observed Castro Tentacles protocol (`castro/api.ts`).
//!
//! Every request is signed at send time (the HMAC covers the `Date` header),
//! paced by one process-wide [`RequestPacer`] per credential set, bounded to
//! 5 MiB, and timed out after 15 s. GETs retry transient failures twice
//! (200 ms, 400 ms); writes are single-attempt because they are not
//! HTTP-idempotent. Under [`SideEffectMode::Record`] writes are recorded and
//! never sent.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use omni_http::public::PublicHttpClient;
use omni_http::{HttpError, Method, SideEffectMode, Url};
use serde::de::DeserializeOwned;

use super::auth::{CastroCredentials, CastroRequestToSign, create_castro_auth_headers};
use super::protocol::{
    self, CastroAction, CastroEpisode, CastroEpisodeSearchResult, CastroEventsResponse,
    CastroPodcast, CastroPodcastSearchResult, CastroPodcastState, CastroProfileSubscription,
    CastroQueue, CastroSubscribedFeed, CastroSubscriptionResponse, CastroSyncStatus,
    CastroUserEventsResponse, ProtocolError, Validate,
};
use crate::pacing::RequestPacer;

const LOG: &str = "Castro";
pub const CASTRO_ORIGIN: &str = "https://tentacles.castro.fm";
const CASTRO_ACCEPT: &str = "application/vnd.tentacles.supertop.co+json; version=8";
const CASTRO_USER_AGENT: &str = "Castro/2396 CFNetwork/3890.100.1 Darwin/27.0.0";
pub const CASTRO_RESPONSE_MAX_BYTES: usize = 5 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CONCURRENT_REQUESTS: usize = 4;
const MAX_REQUESTS_PER_INTERVAL: u32 = 8;
const RATE_INTERVAL: Duration = Duration::from_secs(1);
const GET_RETRIES: u32 = 2;
const RETRY_BASE: Duration = Duration::from_millis(200);

/// `encodeURIComponent` that also escapes `!'()*` (Castro's query encoding).
pub fn encode_castro_query_value(value: &str) -> String {
    let mut out = String::new();
    for c in omni_core::js::encode_uri_component(value).chars() {
        match c {
            '!' | '\'' | '(' | ')' | '*' => out.push_str(&format!("%{:X}", u32::from(c))),
            other => out.push(other),
        }
    }
    out
}

/// Why a Castro request failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CastroFailure {
    /// Network failures and timeouts (got `RequestError`/`TimeoutError`).
    #[error("{0}")]
    Http(String),
    /// Local failures with no HTTP status: oversized bodies, blocked or
    /// invalid URLs, unserializable bodies. Never retried, like the plain
    /// `Error`s the TS reader throws.
    #[error("{0}")]
    Local(String),
    #[error("HTTP {status}{}", body.as_ref().map(|b| format!(": {b}")).unwrap_or_default())]
    Status { status: u16, body: Option<String> },
    #[error("{0}")]
    Decode(#[from] ProtocolError),
}

impl CastroFailure {
    /// `isTransientHttpError`: 429, 5xx, and network/timeouts.
    pub fn is_transient(&self) -> bool {
        match self {
            CastroFailure::Http(_) => true,
            CastroFailure::Status { status, .. } => *status == 429 || *status >= 500,
            CastroFailure::Local(_) | CastroFailure::Decode(_) => false,
        }
    }
}

/// `CastroRequestError`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Castro {method} {path_and_query} failed: {cause}")]
pub struct CastroRequestError {
    pub method: &'static str,
    pub path_and_query: String,
    pub cause: CastroFailure,
}

/// A write captured under [`SideEffectMode::Record`].
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedCastroWrite {
    pub path: String,
    pub body: serde_json::Value,
}

/// The read/write surface [`super::client::CastroClient`] needs; implemented by
/// [`CastroApi`] and by test fakes.
pub trait CastroTransport: Send + Sync {
    fn fetch_podcast<'a>(
        &'a self,
        public_id: &'a str,
    ) -> BoxFuture<'a, Result<CastroPodcast, CastroRequestError>>;
    fn fetch_episode<'a>(
        &'a self,
        public_id: &'a str,
    ) -> BoxFuture<'a, Result<CastroEpisode, CastroRequestError>>;
    fn search_podcasts<'a>(
        &'a self,
        term: &'a str,
    ) -> BoxFuture<'a, Result<Vec<CastroPodcastSearchResult>, CastroRequestError>>;
    fn search_episodes<'a>(
        &'a self,
        term: &'a str,
    ) -> BoxFuture<'a, Result<Vec<CastroEpisodeSearchResult>, CastroRequestError>>;
    fn fetch_subscriptions(
        &self,
    ) -> BoxFuture<'_, Result<Vec<CastroProfileSubscription>, CastroRequestError>>;
    fn fetch_queue(&self) -> BoxFuture<'_, Result<CastroQueue, CastroRequestError>>;
    fn fetch_podcast_state<'a>(
        &'a self,
        public_id: &'a str,
    ) -> BoxFuture<'a, Result<CastroPodcastState, CastroRequestError>>;
    fn post_actions(
        &self,
        actions: Vec<CastroAction>,
    ) -> BoxFuture<'_, Result<(), CastroRequestError>>;
    fn subscribe(
        &self,
        feed_ids: Vec<String>,
    ) -> BoxFuture<'_, Result<CastroSubscriptionResponse, CastroRequestError>>;
}

/// The HTTP Castro client.
pub struct CastroApi {
    http: PublicHttpClient,
    credentials: CastroCredentials,
    clock: SharedClock,
    mode: SideEffectMode,
    origin: String,
    max_response_bytes: usize,
    pacer: RequestPacer,
    recorded: Mutex<Vec<RecordedCastroWrite>>,
}

impl std::fmt::Debug for CastroApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CastroApi")
            .field("access_id", &self.credentials.access_id)
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

enum Body<'a> {
    None,
    Json(&'a serde_json::Value),
}

impl CastroApi {
    pub fn new(
        http: PublicHttpClient,
        credentials: CastroCredentials,
        clock: SharedClock,
        mode: SideEffectMode,
    ) -> Self {
        Self {
            http,
            credentials,
            clock,
            mode,
            origin: CASTRO_ORIGIN.to_owned(),
            max_response_bytes: CASTRO_RESPONSE_MAX_BYTES,
            pacer: RequestPacer::new(
                MAX_CONCURRENT_REQUESTS,
                MAX_REQUESTS_PER_INTERVAL,
                RATE_INTERVAL,
            ),
            recorded: Mutex::new(Vec::new()),
        }
    }

    /// Caps response bodies (tests use tiny limits).
    pub fn with_max_response_bytes(self, max: usize) -> Self {
        Self {
            max_response_bytes: max,
            ..self
        }
    }

    /// Writes captured in [`SideEffectMode::Record`].
    pub fn recorded_writes(&self) -> Vec<RecordedCastroWrite> {
        self.recorded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub async fn get_sync_status(&self) -> Result<CastroSyncStatus, CastroRequestError> {
        self.get("/profile/sync/status".to_owned()).await
    }

    pub async fn fetch_events(
        &self,
        since: u64,
        limit: u64,
    ) -> Result<CastroEventsResponse, CastroRequestError> {
        self.get(format!("/profile/events?since={since}&limit={limit}"))
            .await
    }

    pub async fn fetch_user_events(
        &self,
        since: u64,
        limit: u64,
    ) -> Result<CastroUserEventsResponse, CastroRequestError> {
        self.get(format!(
            "/profile/sync/user_events?since={since}&limit={limit}"
        ))
        .await
    }

    pub async fn fetch_podcast(
        &self,
        public_id: &str,
    ) -> Result<CastroPodcast, CastroRequestError> {
        self.get(format!(
            "/podcasts/{}",
            omni_core::js::encode_uri_component(public_id)
        ))
        .await
    }

    pub async fn fetch_episode(
        &self,
        public_id: &str,
    ) -> Result<CastroEpisode, CastroRequestError> {
        self.get(format!(
            "/episodes/{}",
            omni_core::js::encode_uri_component(public_id)
        ))
        .await
    }

    pub async fn search_podcasts(
        &self,
        term: &str,
    ) -> Result<Vec<CastroPodcastSearchResult>, CastroRequestError> {
        self.get(format!(
            "/search?search_term={}",
            encode_castro_query_value(term)
        ))
        .await
    }

    pub async fn search_episodes(
        &self,
        term: &str,
    ) -> Result<Vec<CastroEpisodeSearchResult>, CastroRequestError> {
        self.get(format!(
            "/episode_search?search_term={}",
            encode_castro_query_value(term)
        ))
        .await
    }

    pub async fn fetch_subscriptions(
        &self,
    ) -> Result<Vec<CastroProfileSubscription>, CastroRequestError> {
        self.get("/profile/subscriptions".to_owned()).await
    }

    pub async fn fetch_queue(&self) -> Result<CastroQueue, CastroRequestError> {
        self.get("/profile/sync/queue".to_owned()).await
    }

    pub async fn fetch_podcast_state(
        &self,
        public_id: &str,
    ) -> Result<CastroPodcastState, CastroRequestError> {
        self.get(format!(
            "/profile/sync/podcast_state?podcast_id={}",
            omni_core::js::encode_uri_component(public_id)
        ))
        .await
    }

    pub async fn post_actions(&self, actions: Vec<CastroAction>) -> Result<(), CastroRequestError> {
        let body = serde_json::json!({ "actions": actions });
        self.write("/profile/sync/actions", &body).await.map(|_| ())
    }

    pub async fn subscribe(
        &self,
        feed_ids: Vec<String>,
    ) -> Result<CastroSubscriptionResponse, CastroRequestError> {
        let path = "/profile/subscriptions/subscribe";
        let body = serde_json::json!({ "feed_ids": feed_ids });
        if self.mode == SideEffectMode::Record {
            self.record(path, &body);
            return Ok(CastroSubscriptionResponse {
                subscribed: feed_ids
                    .into_iter()
                    .map(|feed_id| CastroSubscribedFeed {
                        feed_id,
                        feed_url: String::new(),
                    })
                    .collect(),
                latest_event_id: 0,
            });
        }
        let text = self.write(path, &body).await?;
        decode::<CastroSubscriptionResponse>("POST", path, &text)
    }

    pub async fn unsubscribe(&self, feed_ids: Vec<String>) -> Result<(), CastroRequestError> {
        let body = serde_json::json!({ "feed_ids": feed_ids });
        self.write("/profile/subscriptions/unsubscribe", &body)
            .await
            .map(|_| ())
    }

    fn record(&self, path: &str, body: &serde_json::Value) {
        tracing::info!(target: LOG, path, "Recorded Castro write (side effects disabled)");
        self.recorded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(RecordedCastroWrite {
                path: path.to_owned(),
                body: body.clone(),
            });
    }

    async fn write(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<String, CastroRequestError> {
        if self.mode == SideEffectMode::Record {
            self.record(path, body);
            return Ok(String::new());
        }
        self.attempt(Method::POST, path, Body::Json(body))
            .await
            .map_err(|cause| CastroRequestError {
                method: "POST",
                path_and_query: path.to_owned(),
                cause,
            })
    }

    async fn get<T: DeserializeOwned + Validate>(
        &self,
        path_and_query: String,
    ) -> Result<T, CastroRequestError> {
        let mut attempt = 0;
        let text = loop {
            match self.attempt(Method::GET, &path_and_query, Body::None).await {
                Ok(text) => break text,
                Err(cause) if cause.is_transient() && attempt < GET_RETRIES => {
                    tokio::time::sleep(RETRY_BASE * 2u32.pow(attempt)).await;
                    attempt += 1;
                }
                Err(cause) => {
                    return Err(CastroRequestError {
                        method: "GET",
                        path_and_query,
                        cause,
                    });
                }
            }
        };
        decode("GET", &path_and_query, &text)
    }

    async fn attempt(
        &self,
        method: Method,
        path_and_query: &str,
        body: Body<'_>,
    ) -> Result<String, CastroFailure> {
        let _permit = self.pacer.acquire().await;
        let body_text = match body {
            Body::None => String::new(),
            Body::Json(value) => {
                serde_json::to_string(value).map_err(|e| CastroFailure::Local(e.to_string()))?
            }
        };
        let date = http_date(self.clock.now_ms());
        let method_name = method.as_str().to_owned();
        let headers = create_castro_auth_headers(
            &self.credentials,
            &CastroRequestToSign {
                method: &method_name,
                path_and_query,
                date: &date,
                body: &body_text,
                content_type: None,
            },
        );
        let url = Url::parse(&format!("{}{path_and_query}", self.origin))
            .map_err(|e| CastroFailure::Local(format!("invalid URL: {e}")))?;
        let is_post = method == Method::POST;
        let mut request = self
            .http
            .request(method, url)
            .timeout(REQUEST_TIMEOUT)
            .header("Authorization", headers.authorization)
            .header("Content-Type", headers.content_type)
            .header("Date", headers.date)
            .header("X-Authorization-Content-SHA256", headers.content_sha256)
            .header("Accept", CASTRO_ACCEPT)
            .header("User-Agent", CASTRO_USER_AGENT)
            .header("X-Tentacles-App", "castro-ios")
            .header("X-Tentacles-Platform", "iOS");
        if is_post {
            request = request.body(body_text);
        }
        let response = request
            .send_bounded(self.max_response_bytes)
            .await
            .map_err(|e| match e {
                HttpError::Status { status, body } => CastroFailure::Status {
                    status,
                    body: non_empty_prefix(&body),
                },
                other if other.is_transient() => CastroFailure::Http(other.to_string()),
                other => CastroFailure::Local(other.to_string()),
            })?;
        let text = String::from_utf8_lossy(&response.body).into_owned();
        let status = response.status.as_u16();
        if !(200..300).contains(&status) {
            return Err(CastroFailure::Status {
                status,
                body: non_empty_prefix(&text),
            });
        }
        Ok(text)
    }
}

fn non_empty_prefix(body: &str) -> Option<String> {
    let prefix = omni_core::js::utf16_slice(body, 0, 200).into_owned();
    (!prefix.is_empty()).then_some(prefix)
}

fn decode<T: DeserializeOwned + Validate>(
    method: &'static str,
    path_and_query: &str,
    text: &str,
) -> Result<T, CastroRequestError> {
    protocol::decode_str(text).map_err(|e| CastroRequestError {
        method,
        path_and_query: path_and_query.to_owned(),
        cause: CastroFailure::Decode(e),
    })
}

/// `Date#toUTCString()`.
pub fn http_date(ms: i64) -> String {
    omni_core::clock::timestamp_from_ms(ms)
        .strftime("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

impl CastroTransport for CastroApi {
    fn fetch_podcast<'a>(
        &'a self,
        public_id: &'a str,
    ) -> BoxFuture<'a, Result<CastroPodcast, CastroRequestError>> {
        Box::pin(CastroApi::fetch_podcast(self, public_id))
    }
    fn fetch_episode<'a>(
        &'a self,
        public_id: &'a str,
    ) -> BoxFuture<'a, Result<CastroEpisode, CastroRequestError>> {
        Box::pin(CastroApi::fetch_episode(self, public_id))
    }
    fn search_podcasts<'a>(
        &'a self,
        term: &'a str,
    ) -> BoxFuture<'a, Result<Vec<CastroPodcastSearchResult>, CastroRequestError>> {
        Box::pin(CastroApi::search_podcasts(self, term))
    }
    fn search_episodes<'a>(
        &'a self,
        term: &'a str,
    ) -> BoxFuture<'a, Result<Vec<CastroEpisodeSearchResult>, CastroRequestError>> {
        Box::pin(CastroApi::search_episodes(self, term))
    }
    fn fetch_subscriptions(
        &self,
    ) -> BoxFuture<'_, Result<Vec<CastroProfileSubscription>, CastroRequestError>> {
        Box::pin(CastroApi::fetch_subscriptions(self))
    }
    fn fetch_queue(&self) -> BoxFuture<'_, Result<CastroQueue, CastroRequestError>> {
        Box::pin(CastroApi::fetch_queue(self))
    }
    fn fetch_podcast_state<'a>(
        &'a self,
        public_id: &'a str,
    ) -> BoxFuture<'a, Result<CastroPodcastState, CastroRequestError>> {
        Box::pin(CastroApi::fetch_podcast_state(self, public_id))
    }
    fn post_actions(
        &self,
        actions: Vec<CastroAction>,
    ) -> BoxFuture<'_, Result<(), CastroRequestError>> {
        Box::pin(CastroApi::post_actions(self, actions))
    }
    fn subscribe(
        &self,
        feed_ids: Vec<String>,
    ) -> BoxFuture<'_, Result<CastroSubscriptionResponse, CastroRequestError>> {
        Box::pin(CastroApi::subscribe(self, feed_ids))
    }
}

/// The cached API and the credential key it was built for.
type SharedApi = (String, Arc<CastroApi>);

/// The process-wide API for one credential set (`sharedCastroApi`): every
/// client, including overlapping runs, funnels through one pacer. New
/// credentials replace the cached API.
pub fn shared_castro_api(
    http: &PublicHttpClient,
    clock: &SharedClock,
    mode: SideEffectMode,
    access_id: &str,
    secret: &str,
) -> Arc<CastroApi> {
    static SHARED: OnceLock<Mutex<Option<SharedApi>>> = OnceLock::new();
    let key = format!("{access_id}\0{secret}");
    let mut slot = SHARED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some((cached_key, api)) = slot.as_ref()
        && *cached_key == key
    {
        return api.clone();
    }
    let api = Arc::new(CastroApi::new(
        http.clone(),
        CastroCredentials {
            access_id: access_id.to_owned(),
            secret: secret.as_bytes().to_vec(),
        },
        clock.clone(),
        mode,
    ));
    *slot = Some((key, api.clone()));
    api
}
