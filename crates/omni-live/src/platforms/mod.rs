//! Platform status fetchers (`platforms/*`).

mod kick;
mod twitch;
mod youtube;

use futures::future::BoxFuture;
use omni_http::public::PublicHttpClient;
use omni_http::{HttpError, RequestBuilder};

pub use kick::{
    KICK_CHANNELS_MAX_BYTES, KICK_TOKEN_MAX_BYTES, KickCategory, KickChannel, KickChannelsResponse,
    KickClient, KickCredentials, KickStream, extract_kick_status,
};
pub use twitch::{
    TwitchBroadcastSettings, TwitchClient, TwitchData, TwitchGame, TwitchGqlResponse, TwitchStream,
    TwitchUser, extract_twitch_status, twitch_query,
};
pub use youtube::{YouTubeClient, extract_youtube_status};

use crate::platform::{FetchedStatus, Platform, PlatformBinding};

/// Platform request timeout.
pub(crate) const PLATFORM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// GQL / API JSON response cap.
pub const PLATFORM_GQL_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Live-page HTML cap.
pub const PLATFORM_HTML_MAX_BYTES: usize = 10 * 1024 * 1024;

/// Checks one binding; never fails (failures are `Unknown`).
pub trait StatusFetcher: Send + Sync {
    fn fetch<'a>(&'a self, binding: &'a PlatformBinding) -> BoxFuture<'a, FetchedStatus>;
}

/// The production fetchers over the public-address HTTP client.
pub struct Platforms {
    twitch: TwitchClient,
    kick: KickClient,
    youtube: YouTubeClient,
}

impl Platforms {
    pub fn new(http: PublicHttpClient, kick: Option<KickCredentials>) -> Self {
        Self {
            twitch: TwitchClient::new(http.clone()),
            kick: KickClient::new(http.clone(), kick),
            youtube: YouTubeClient::new(http),
        }
    }

    /// Kick token expiry follows this clock.
    pub fn with_clock(mut self, clock: omni_core::clock::SharedClock) -> Self {
        self.kick = self.kick.with_clock(clock);
        self
    }

    pub fn twitch(&self) -> &TwitchClient {
        &self.twitch
    }

    pub fn kick(&self) -> &KickClient {
        &self.kick
    }

    pub fn youtube(&self) -> &YouTubeClient {
        &self.youtube
    }
}

impl StatusFetcher for Platforms {
    fn fetch<'a>(&'a self, binding: &'a PlatformBinding) -> BoxFuture<'a, FetchedStatus> {
        Box::pin(async move {
            match binding.platform {
                Platform::Twitch => self.twitch.fetch_live_status(&binding.username).await,
                Platform::Kick => self.kick.fetch_live_status(&binding.username).await,
                Platform::YouTube => self.youtube.fetch_live_status(&binding.username).await,
            }
        })
    }
}

/// `PlatformRequestError.message`: `<operation>: <detail>`.
pub(crate) fn request_failure(operation: &str, error: &HttpError) -> String {
    let detail = match error {
        HttpError::TooLarge { limit } => format!("Response exceeds the {limit}-byte limit"),
        HttpError::Timeout => "Timeout awaiting 'request'".to_owned(),
        other => other.to_string(),
    };
    format!("{operation}: {detail}")
}

/// Sends `request`; a non-2xx status is an error.
pub(crate) async fn fetch_text(
    request: RequestBuilder,
    max_bytes: usize,
) -> Result<String, HttpError> {
    let response = request
        .timeout(PLATFORM_TIMEOUT)
        .send_bounded(max_bytes)
        .await?;
    if !response.status.is_success() {
        return Err(HttpError::Status {
            status: response.status.as_u16(),
            body: String::new(),
        });
    }
    Ok(String::from_utf8_lossy(&response.body).into_owned())
}

/// A failed platform request (`PlatformRequestError`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PlatformRequestError {
    pub message: String,
}

/// POST `{query}` with a client id, bounded, decoded into `T`.
pub async fn fetch_gql<T: serde::de::DeserializeOwned>(
    http: &PublicHttpClient,
    url: &str,
    client_id: &str,
    query: &str,
    max_bytes: usize,
) -> Result<T, PlatformRequestError> {
    let operation = format!("Failed to fetch GQL from {url}");
    let fail = |message: String| PlatformRequestError { message };
    let parsed = url::Url::parse(url).map_err(|e| fail(format!("{operation}: {e}")))?;
    let request = http
        .request(omni_http::Method::POST, parsed)
        .header("Client-Id", client_id)
        .json(&serde_json::json!({ "query": query }));
    let body = fetch_text(request, max_bytes)
        .await
        .map_err(|e| fail(request_failure(&operation, &e)))?;
    serde_json::from_str(&body).map_err(|e| fail(format!("{operation}: {e}")))
}

/// GET a public page, bounded.
pub async fn fetch_page_html(
    http: &PublicHttpClient,
    url: &str,
    max_bytes: usize,
) -> Result<String, PlatformRequestError> {
    let operation = format!("Failed to check live status for {url}");
    let fail = |message: String| PlatformRequestError { message };
    let parsed = url::Url::parse(url).map_err(|e| fail(format!("{operation}: {e}")))?;
    fetch_text(http.request(omni_http::Method::GET, parsed), max_bytes)
        .await
        .map_err(|e| fail(request_failure(&operation, &e)))
}
