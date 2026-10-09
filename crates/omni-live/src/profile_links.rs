//! Deterministic profile identity evidence (`profileLinks.ts`).
//!
//! A DGG-discovered account is linked to a configured account only through
//! YouTube's oEmbed video owner, a direct profile link with equal handles, or
//! reciprocal direct profile links. Requests refuse redirects and are bounded.

use std::sync::LazyLock;
use std::time::Duration;

use futures::future::BoxFuture;
use indexmap::IndexMap;
use omni_http::{HttpClient, HttpError, Method, RedirectRule};
use omni_store::Store;
use regex::Regex;
use scraper::{Html, Selector};
use serde::Deserialize;
use url::Url;

use crate::error::LiveError;
use crate::identity::{ProfileIdentityLink, binding_key, get_link, remember_link};
use crate::platform::{Platform, PlatformBinding};
use crate::streamers::js_trim;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MAX_BYTES: usize = 2_000_000;
const RESERVED_KICK_PATHS: [&str; 6] = [
    "api",
    "browse",
    "categories",
    "category",
    "following",
    "search",
];
const RESERVED_TWITCH_PATHS: [&str; 6] =
    ["directory", "downloads", "jobs", "p", "settings", "videos"];

#[allow(clippy::expect_used)]
static HANDLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]{2,100}$").expect("valid regex"));
#[allow(clippy::expect_used)]
static VIDEO_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{11}$").expect("valid regex"));
#[allow(clippy::expect_used)]
static EMBEDDED_PROFILE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)https?:\\?/\\?/(?:www\\?\.)?(?:youtube\\?\.com|kick\\?\.com|twitch\\?\.tv)[^"'<>\s]*"#)
        .expect("valid regex")
});
#[allow(clippy::expect_used)]
static ANCHORS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").expect("valid selector"));

/// A failed profile or oEmbed lookup: `<operation>: <detail>`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{operation}: {detail}")]
pub struct ProfileLinkError {
    pub operation: String,
    pub detail: String,
}

/// Why learning an identity failed (the task retries later either way).
#[derive(Debug, thiserror::Error)]
pub enum LearnError {
    #[error(transparent)]
    Link(#[from] ProfileLinkError),
    #[error(transparent)]
    Persistence(#[from] LiveError),
}

fn valid_handle(value: &str) -> bool {
    HANDLE.is_match(value)
}

fn unwrap_youtube_redirect(url: Url) -> Option<Url> {
    let host = url.host_str().unwrap_or_default();
    if (host == "youtube.com" || host == "www.youtube.com") && url.path() == "/redirect" {
        let destination = url
            .query_pairs()
            .find(|(key, _)| key == "q")
            .map(|(_, value)| value.into_owned())?;
        if destination.is_empty() {
            return None;
        }
        return Url::parse(&destination).ok();
    }
    Some(url)
}

/// Account/profile URLs only; videos, categories and other links are rejected.
pub fn binding_from_profile_url(raw: &str) -> Option<PlatformBinding> {
    let decoded = html_escape::decode_html_entities(raw).replace("\\/", "/");
    let url = unwrap_youtube_redirect(Url::parse(&decoded).ok()?)?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let parts: Vec<&str> = url.path().split('/').filter(|p| !p.is_empty()).collect();

    match (host, parts.as_slice()) {
        ("kick.com", [username]) => {
            if !valid_handle(username)
                || RESERVED_KICK_PATHS.contains(&username.to_lowercase().as_str())
            {
                return None;
            }
            Some(PlatformBinding::new(
                Platform::Kick,
                username.to_lowercase(),
            ))
        }
        ("twitch.tv", [username]) => {
            if !valid_handle(username)
                || RESERVED_TWITCH_PATHS.contains(&username.to_lowercase().as_str())
            {
                return None;
            }
            Some(PlatformBinding::new(
                Platform::Twitch,
                username.to_lowercase(),
            ))
        }
        ("youtube.com", [single]) => {
            let handle = single.strip_prefix('@')?;
            valid_handle(handle).then(|| {
                PlatformBinding::new(Platform::YouTube, format!("@{}", handle.to_lowercase()))
            })
        }
        ("youtube.com", [kind, value]) => {
            let kind = kind.to_lowercase();
            (["channel", "c", "user"].contains(&kind.as_str()) && valid_handle(value))
                .then(|| PlatformBinding::new(Platform::YouTube, format!("{kind}/{value}")))
        }
        _ => None,
    }
}

/// Direct supported profile links from anchors and embedded page JSON.
pub fn extract_profile_links(html: &str) -> Vec<PlatformBinding> {
    let mut candidates: Vec<String> = Html::parse_document(html)
        .select(&ANCHORS)
        .filter_map(|anchor| anchor.value().attr("href"))
        .filter(|href| !href.is_empty())
        .map(str::to_owned)
        .collect();
    // YouTube and Kick serialize external links into page JSON as escaped URLs.
    candidates.extend(
        EMBEDDED_PROFILE_URL
            .find_iter(html)
            .map(|m| m.as_str().to_owned()),
    );

    let mut by_key: IndexMap<String, PlatformBinding> = IndexMap::new();
    for candidate in candidates {
        if let Some(binding) = binding_from_profile_url(&candidate) {
            by_key.insert(binding_key(&binding), binding);
        }
    }
    by_key.into_values().collect()
}

/// The canonical profile page of a supported account.
pub fn profile_page_url(binding: &PlatformBinding) -> Option<String> {
    let username = js_trim(&binding.username);
    if username.is_empty() {
        return None;
    }
    match binding.platform {
        Platform::YouTube => {
            if let Some(handle) = username.strip_prefix('@')
                && valid_handle(handle)
            {
                return Some(format!(
                    "https://www.youtube.com/@{}/about",
                    handle.to_lowercase()
                ));
            }
            let parts: Vec<&str> = username.split('/').collect();
            match parts.as_slice() {
                [kind, value]
                    if ["channel", "c", "user"].contains(&kind.to_lowercase().as_str())
                        && valid_handle(value) =>
                {
                    Some(format!(
                        "https://www.youtube.com/{}/{value}/about",
                        kind.to_lowercase()
                    ))
                }
                _ => None,
            }
        }
        Platform::Kick => {
            valid_handle(username).then(|| format!("https://kick.com/{}", username.to_lowercase()))
        }
        Platform::Twitch => valid_handle(username)
            .then(|| format!("https://www.twitch.tv/{}/about", username.to_lowercase())),
    }
}

fn normalized_handle(binding: &PlatformBinding) -> Option<String> {
    let lower = js_trim(&binding.username).to_lowercase();
    let handle = lower.strip_prefix('@').unwrap_or(&lower);
    (!handle.contains('/')).then(|| handle.to_owned())
}

/// The confidence gate before any durable alias is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileIdentityEvidence {
    EqualHandle,
    Reciprocal,
}

pub fn profile_identity_evidence(
    source: &PlatformBinding,
    target: &PlatformBinding,
    direct_links: &[PlatformBinding],
    reciprocal_links: &[PlatformBinding],
) -> Option<ProfileIdentityEvidence> {
    let source_key = binding_key(source);
    let target_key = binding_key(target);
    if !direct_links
        .iter()
        .any(|link| binding_key(link) == target_key)
    {
        return None;
    }
    let source_handle = normalized_handle(source);
    if source_handle.is_some() && source_handle == normalized_handle(target) {
        return Some(ProfileIdentityEvidence::EqualHandle);
    }
    reciprocal_links
        .iter()
        .any(|link| binding_key(link) == source_key)
        .then_some(ProfileIdentityEvidence::Reciprocal)
}

#[derive(Deserialize)]
struct VideoOwner {
    #[serde(rename = "type")]
    kind: String,
    author_url: String,
}

/// Bounded, redirect-refusing page and oEmbed reads.
#[derive(Clone)]
pub struct ProfileFetcher {
    http: HttpClient,
    timeout: Duration,
    max_bytes: usize,
}

impl ProfileFetcher {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            timeout: DEFAULT_TIMEOUT,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    async fn read_bounded(
        &self,
        url: Url,
        accept: &str,
        operation: &str,
    ) -> Result<String, ProfileLinkError> {
        let fail = |detail: String| ProfileLinkError {
            operation: operation.to_owned(),
            detail,
        };
        let response = self
            .http
            .request(Method::GET, url)
            .header("Accept", accept)
            .redirect(RedirectRule::Error)
            .timeout(self.timeout)
            .send_bounded(self.max_bytes)
            .await
            .map_err(|error| match error {
                HttpError::TooLarge { limit } => {
                    fail(format!("Profile page exceeds {limit} byte limit"))
                }
                other => fail(other.to_string()),
            })?;
        if !response.status.is_success() {
            return Err(fail(format!(
                "Profile page returned HTTP {}",
                response.status.as_u16()
            )));
        }
        Ok(String::from_utf8_lossy(&response.body).into_owned())
    }

    /// The direct profile links on exactly the canonical page of `binding`.
    pub async fn fetch_profile_links(
        &self,
        binding: &PlatformBinding,
    ) -> Result<Vec<PlatformBinding>, ProfileLinkError> {
        let Some(page) = profile_page_url(binding) else {
            return Ok(Vec::new());
        };
        let operation = format!("fetch profile page {page}");
        let url = Url::parse(&page).map_err(|e| ProfileLinkError {
            operation: operation.clone(),
            detail: e.to_string(),
        })?;
        let html = self
            .read_bounded(url, "text/html,application/xhtml+xml", &operation)
            .await?;
        Ok(extract_profile_links(&html))
    }

    /// The owning channel of a YouTube video, from oEmbed metadata.
    pub async fn fetch_youtube_video_owner(
        &self,
        video_id: &str,
    ) -> Result<Option<PlatformBinding>, ProfileLinkError> {
        if !VIDEO_ID.is_match(video_id) {
            return Ok(None);
        }
        let operation = format!("fetch YouTube video owner {video_id}");
        let fail = |detail: String| ProfileLinkError {
            operation: operation.clone(),
            detail,
        };
        let mut url =
            Url::parse("https://www.youtube.com/oembed").map_err(|e| fail(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair(
                "url",
                &format!("https://www.youtube.com/watch?v={video_id}"),
            )
            .append_pair("format", "json");
        let body = self
            .read_bounded(url, "application/json", &operation)
            .await
            .map_err(|e| fail(e.detail))?;
        let owner: VideoOwner = serde_json::from_str(&body).map_err(|e| fail(e.to_string()))?;
        if owner.kind != "video" {
            return Err(fail(format!(
                "expected type \"video\", got {:?}",
                owner.kind
            )));
        }
        Ok(binding_from_profile_url(&owner.author_url).filter(|b| b.platform == Platform::YouTube))
    }
}

/// What to learn and against which configured accounts.
#[derive(Clone, Debug)]
pub struct LearnInput {
    pub source: PlatformBinding,
    pub configured_bindings: Vec<PlatformBinding>,
    pub now: i64,
    pub force_refresh: bool,
}

/// Learns (or revalidates) a durable source-to-configured alias.
pub trait IdentityLearner: Send + Sync {
    fn learn(
        &self,
        input: LearnInput,
    ) -> BoxFuture<'_, Result<Option<ProfileIdentityLink>, LearnError>>;
}

/// Production learner over [`ProfileFetcher`] and the docstore.
#[derive(Clone)]
pub struct ProfileIdentityLearner {
    store: Store,
    fetcher: ProfileFetcher,
}

impl ProfileIdentityLearner {
    pub fn new(store: Store, fetcher: ProfileFetcher) -> Self {
        Self { store, fetcher }
    }

    pub async fn learn_identity(
        &self,
        input: LearnInput,
    ) -> Result<Option<ProfileIdentityLink>, LearnError> {
        let configured: IndexMap<String, &PlatformBinding> = input
            .configured_bindings
            .iter()
            .map(|binding| (binding_key(binding), binding))
            .collect();
        let existing = get_link(&self.store, &input.source).await?;
        if !input.force_refresh
            && let Some(existing) = existing
            && configured.contains_key(&existing.target_binding)
        {
            return Ok(Some(existing));
        }
        let source_key = binding_key(&input.source);
        let source = &input.source;

        if source.platform == Platform::YouTube && VIDEO_ID.is_match(&source.username) {
            let owner = self
                .fetcher
                .fetch_youtube_video_owner(&source.username)
                .await?;
            let target = owner.and_then(|owner| configured.get(&binding_key(&owner)).copied());
            return match target {
                Some(target) if binding_key(target) != source_key => Ok(Some(
                    remember_link(&self.store, source, target, input.now).await?,
                )),
                _ => Ok(None),
            };
        }

        let direct_links = self.fetcher.fetch_profile_links(source).await?;
        for direct in &direct_links {
            let Some(target) = configured.get(&binding_key(direct)).copied() else {
                continue;
            };
            if binding_key(target) == source_key {
                continue;
            }
            if profile_identity_evidence(source, target, &direct_links, &[])
                == Some(ProfileIdentityEvidence::EqualHandle)
            {
                return Ok(Some(
                    remember_link(&self.store, source, target, input.now).await?,
                ));
            }
            let reciprocal = self.fetcher.fetch_profile_links(target).await?;
            if profile_identity_evidence(source, target, &direct_links, &reciprocal)
                == Some(ProfileIdentityEvidence::Reciprocal)
            {
                return Ok(Some(
                    remember_link(&self.store, source, target, input.now).await?,
                ));
            }
        }
        Ok(None)
    }
}

impl IdentityLearner for ProfileIdentityLearner {
    fn learn(
        &self,
        input: LearnInput,
    ) -> BoxFuture<'_, Result<Option<ProfileIdentityLink>, LearnError>> {
        Box::pin(self.learn_identity(input))
    }
}
