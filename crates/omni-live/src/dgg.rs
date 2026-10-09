//! Destiny.gg discovery (`dgg.ts`): the live websocket snapshot and its
//! resolution into configured-streamer enrichment plus transient discoveries.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use omni_api::streamers::{DggPresence, StreamerTier};
use serde::Deserialize;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::identity::canonical_binding_key;
use crate::platform::{FetchedLive, Platform, PlatformBinding};
use crate::streamers::{DiscoverySource, Streamer, normalize_id};

/// The DGG live websocket.
pub const DGG_LIVE_URL: &str = "wss://live.destiny.gg";
const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// `dggApi:embeds` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DggEmbed {
    pub platform: String,
    pub id: String,
    pub count: i64,
    pub media_platform: String,
    pub media_id: String,
    pub metadata: DggMediaMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DggMediaMetadata {
    pub preview_url: Option<String>,
    pub display_name: String,
    pub title: Option<String>,
    pub created_date: Option<String>,
    pub live: bool,
    pub viewers: Option<i64>,
}

/// `dggApi:hosting` (non-null).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DggHosting {
    pub id: String,
    pub display_name: String,
    pub platform: String,
    pub title: Option<String>,
    pub preview: Option<String>,
}

/// One complete DGG snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DggFeed {
    pub embeds: Vec<DggEmbed>,
    pub hosting: Option<DggHosting>,
    pub destiny_live: bool,
}

/// A failed or incomplete snapshot.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DggFeedError {
    pub message: String,
}

impl DggFeedError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

// --- payload validation (zod schemas with passthrough) ---------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEmbed {
    platform: String,
    id: String,
    count: f64,
    media_item: RawMediaItem,
}

#[derive(Deserialize)]
struct RawMediaItem {
    identifier: RawIdentifier,
    metadata: RawMetadata,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIdentifier {
    platform: String,
    media_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMetadata {
    #[serde(default)]
    preview_url: Option<String>,
    display_name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    created_date: Option<String>,
    live: bool,
    #[serde(default)]
    viewers: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHosting {
    id: String,
    display_name: String,
    platform: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    preview: Option<String>,
}

#[derive(Deserialize)]
struct RawStreamInfo {
    streams: HashMap<String, Option<RawStream>>,
}

#[derive(Deserialize)]
struct RawStream {
    #[serde(default)]
    live: Option<bool>,
}

fn non_negative_int(value: f64, field: &str) -> Result<i64, String> {
    if value.fract() != 0.0 || !value.is_finite() {
        return Err(format!("{field}: expected an integer"));
    }
    if value < 0.0 {
        return Err(format!("{field}: expected a non-negative number"));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(value as i64)
}

fn non_empty(value: String, field: &str) -> Result<String, String> {
    if value.is_empty() {
        Err(format!("{field}: must contain at least 1 character"))
    } else {
        Ok(value)
    }
}

/// `dggEmbedsSchema.parse`.
pub fn parse_embeds(data: Value) -> Result<Vec<DggEmbed>, String> {
    let raw: Vec<RawEmbed> = serde_json::from_value(data).map_err(|e| e.to_string())?;
    raw.into_iter()
        .map(|embed| {
            let metadata = embed.media_item.metadata;
            Ok(DggEmbed {
                platform: embed.platform,
                id: non_empty(embed.id, "id")?,
                count: non_negative_int(embed.count, "count")?,
                media_platform: embed.media_item.identifier.platform,
                media_id: non_empty(embed.media_item.identifier.media_id, "mediaId")?,
                metadata: DggMediaMetadata {
                    preview_url: metadata.preview_url,
                    display_name: non_empty(metadata.display_name, "displayName")?,
                    title: metadata.title,
                    created_date: metadata.created_date,
                    live: metadata.live,
                    viewers: metadata
                        .viewers
                        .map(|v| non_negative_int(v, "viewers"))
                        .transpose()?,
                },
            })
        })
        .collect()
}

/// `dggHostingSchema.parse` (nullable).
pub fn parse_hosting(data: Value) -> Result<Option<DggHosting>, String> {
    let raw: Option<RawHosting> = serde_json::from_value(data).map_err(|e| e.to_string())?;
    raw.map(|hosting| {
        Ok(DggHosting {
            id: non_empty(hosting.id, "id")?,
            display_name: non_empty(hosting.display_name, "displayName")?,
            platform: hosting.platform,
            title: hosting.title,
            preview: hosting.preview,
        })
    })
    .transpose()
}

/// `dggStreamInfoSchema.parse` reduced to "is any Destiny stream live".
pub fn parse_destiny_live(data: Value) -> Result<bool, String> {
    let raw: RawStreamInfo = serde_json::from_value(data).map_err(|e| e.to_string())?;
    Ok(raw
        .streams
        .values()
        .any(|stream| stream.as_ref().and_then(|s| s.live) == Some(true)))
}

/// Collects the three snapshot messages in any order. Every relevant message
/// is required (a null hosting message included), so a partial snapshot
/// never erases the last successful channel list.
#[derive(Debug, Default)]
pub struct SnapshotAssembler {
    embeds: Option<Vec<DggEmbed>>,
    hosting: Option<Option<DggHosting>>,
    destiny_live: Option<bool>,
}

impl SnapshotAssembler {
    /// Feeds one text frame: `Ok(Some(feed))` once complete, `Err` for an
    /// invalid relevant payload; irrelevant or non-JSON frames are ignored.
    pub fn accept(&mut self, text: &str) -> Result<Option<DggFeed>, DggFeedError> {
        let Ok(Value::Object(mut envelope)) = serde_json::from_str::<Value>(text) else {
            return Ok(None);
        };
        let Some(Value::String(kind)) = envelope.remove("type") else {
            return Ok(None);
        };
        let invalid =
            |message: String| DggFeedError::new(format!("Invalid {kind} payload: {message}"));
        // A missing `data` member is `undefined`, which every schema rejects
        // (the hosting schema accepts `null`, not an absent payload).
        let data = envelope.remove("data");
        let required = |data: Option<Value>| data.ok_or_else(|| "Required".to_owned());
        match kind.as_str() {
            "dggApi:embeds" => {
                self.embeds = Some(required(data).and_then(parse_embeds).map_err(invalid)?);
            }
            "dggApi:hosting" => {
                self.hosting = Some(required(data).and_then(parse_hosting).map_err(invalid)?);
            }
            "dggApi:streamInfo" => {
                self.destiny_live = Some(
                    required(data)
                        .and_then(parse_destiny_live)
                        .map_err(invalid)?,
                );
            }
            _ => return Ok(None),
        }
        Ok(self.complete())
    }

    fn complete(&self) -> Option<DggFeed> {
        let (Some(embeds), Some(hosting), Some(destiny_live)) =
            (&self.embeds, &self.hosting, self.destiny_live)
        else {
            return None;
        };
        Some(DggFeed {
            embeds: embeds.clone(),
            // A stale host is suppressed while Destiny is live.
            hosting: if destiny_live { None } else { hosting.clone() },
            destiny_live,
        })
    }
}

/// Produces one DGG snapshot.
pub trait DggFeedSource: Send + Sync {
    fn fetch(&self) -> BoxFuture<'_, Result<DggFeed, DggFeedError>>;
}

/// The websocket snapshot reader (read-only; no side effects).
#[derive(Clone, Debug)]
pub struct WebSocketDggFeed {
    url: String,
    timeout: Duration,
}

impl Default for WebSocketDggFeed {
    fn default() -> Self {
        Self {
            url: DGG_LIVE_URL.to_owned(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl WebSocketDggFeed {
    /// A reader for another endpoint (tests run an in-process server).
    pub fn new(url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            url: url.into(),
            timeout,
        }
    }

    async fn read_snapshot(&self) -> Result<DggFeed, DggFeedError> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|_| DggFeedError::new("DGG websocket connection failed"))?;
        request.headers_mut().insert(
            http_header::USER_AGENT,
            http_header::HeaderValue::from_static(omni_http::USER_AGENT),
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let (mut socket, _) =
            tokio_tungstenite::connect_async_with_config(request, Some(config), false)
                .await
                .map_err(|_| DggFeedError::new("DGG websocket connection failed"))?;
        let mut assembler = SnapshotAssembler::default();
        let outcome = loop {
            let Some(frame) = socket.next().await else {
                break Err(DggFeedError::new(
                    "DGG websocket closed before its snapshot arrived",
                ));
            };
            let text = match frame {
                Ok(Message::Text(text)) => text.as_str().to_owned(),
                Ok(Message::Binary(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
                Ok(Message::Close(_)) => {
                    break Err(DggFeedError::new(
                        "DGG websocket closed before its snapshot arrived",
                    ));
                }
                Ok(_) => continue,
                Err(_) => break Err(DggFeedError::new("DGG websocket connection failed")),
            };
            match assembler.accept(&text) {
                Ok(Some(feed)) => break Ok(feed),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
        };
        // Best effort: the snapshot is complete either way.
        if socket.close(None).await.is_err() {
            tracing::debug!(target: "LiveCheckTask", "DGG websocket close failed");
        }
        outcome
    }
}

mod http_header {
    pub use tokio_tungstenite::tungstenite::http::HeaderValue;
    pub use tokio_tungstenite::tungstenite::http::header::USER_AGENT;
}

impl DggFeedSource for WebSocketDggFeed {
    fn fetch(&self) -> BoxFuture<'_, Result<DggFeed, DggFeedError>> {
        Box::pin(async move {
            match tokio::time::timeout(self.timeout, self.read_snapshot()).await {
                Ok(result) => result,
                Err(_) => Err(DggFeedError::new("Timed out waiting for DGG live snapshot")),
            }
        })
    }
}

// --- resolution -------------------------------------------------------------

/// A DGG source selected as a discovery or attached to a configured identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedDggStream {
    pub streamer: Streamer,
    pub status: FetchedLive,
    /// Exact media/channel URL of the entry.
    pub url: String,
    pub preview_url: Option<String>,
    pub embed_count: Option<i64>,
    pub hosted: bool,
    pub hosting: Option<DggHosting>,
}

/// One snapshot resolved against the configured streamers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedDggStreams {
    /// Unconfigured sources that become transient background streamers.
    pub discovered: Vec<SelectedDggStream>,
    /// DGG presence overlaid on configured streamers (by streamer id).
    pub configured_presence: IndexMap<String, DggPresence>,
    /// Alias-linked platform observations of a configured identity.
    pub configured_sources: IndexMap<String, Vec<SelectedDggStream>>,
}

/// `canonicalBinding(platform, username)`.
pub fn canonical_binding(platform: Platform, username: &str) -> String {
    canonical_binding_key(platform, username)
}

fn supported_platform(value: &str) -> Option<Platform> {
    Platform::parse(&value.to_lowercase())
}

fn stream_url(platform: Platform, id: &str) -> String {
    match platform {
        Platform::YouTube => format!(
            "https://www.youtube.com/watch?v={}",
            omni_core::js::encode_uri_component(id)
        ),
        other => other.live_url(id),
    }
}

struct Candidate {
    platform: Platform,
    id: String,
    display_name: String,
    title: String,
    viewers: Option<i64>,
    started_at: Option<String>,
    preview_url: Option<String>,
    embed_count: Option<i64>,
    hosting: Option<DggHosting>,
}

fn sort_by_count_desc<T>(items: &mut [T], count: impl Fn(&T) -> i64) {
    items.sort_by_key(|item| std::cmp::Reverse(count(item)));
}

/// Resolves a snapshot into enrichment of configured streamers and up to
/// `limit` discoveries. Configured matches never consume a discovery slot;
/// YouTube ownership is never inferred from display names.
pub fn resolve_dgg_streams(
    feed: &DggFeed,
    limit: usize,
    configured_streamers: &[Streamer],
    available_platforms: &HashSet<Platform>,
    identity_aliases: &HashMap<String, String>,
) -> ResolvedDggStreams {
    let mut configured_by_binding: HashMap<String, &Streamer> = HashMap::new();
    let mut configured_by_name: HashMap<String, &Streamer> = HashMap::new();
    for streamer in configured_streamers {
        configured_by_name.insert(normalize_id(&streamer.display_name), streamer);
        for binding in &streamer.bindings {
            configured_by_binding.insert(
                canonical_binding(binding.platform, &binding.username),
                streamer,
            );
        }
    }

    let mut candidates = Vec::new();
    if !feed.destiny_live
        && let Some(hosting) = &feed.hosting
        && let Some(platform) = supported_platform(&hosting.platform)
        && available_platforms.contains(&platform)
    {
        let host_key = canonical_binding(platform, &hosting.id);
        let mut matching: Vec<&DggEmbed> = feed
            .embeds
            .iter()
            .filter(|embed| {
                supported_platform(&embed.platform) == Some(platform)
                    && canonical_binding(platform, &embed.id) == host_key
            })
            .collect();
        sort_by_count_desc(&mut matching, |embed| embed.count);
        let matching = matching.first();
        candidates.push(Candidate {
            platform,
            id: hosting.id.clone(),
            display_name: hosting.display_name.clone(),
            title: hosting
                .title
                .clone()
                .unwrap_or_else(|| format!("{} is hosted on DGG", hosting.display_name)),
            viewers: matching.and_then(|embed| embed.metadata.viewers),
            started_at: matching.and_then(|embed| embed.metadata.created_date.clone()),
            preview_url: hosting.preview.clone(),
            embed_count: matching.map(|embed| embed.count),
            hosting: Some(hosting.clone()),
        });
    }

    let mut embeds: Vec<&DggEmbed> = feed.embeds.iter().collect();
    sort_by_count_desc(&mut embeds, |embed| embed.count);
    for embed in embeds {
        let Some(platform) = supported_platform(&embed.platform) else {
            continue;
        };
        if supported_platform(&embed.media_platform) != Some(platform)
            || !available_platforms.contains(&platform)
            || !embed.metadata.live
        {
            continue;
        }
        candidates.push(Candidate {
            platform,
            id: embed.media_id.clone(),
            display_name: embed.metadata.display_name.clone(),
            title: embed
                .metadata
                .title
                .clone()
                .unwrap_or_else(|| embed.metadata.display_name.clone()),
            viewers: embed.metadata.viewers,
            started_at: embed.metadata.created_date.clone(),
            preview_url: embed.metadata.preview_url.clone(),
            embed_count: Some(embed.count),
            hosting: None,
        });
    }

    // Hosting is one more placement, not a priority lane: rank everything by
    // DGG viewers (stable, so the richer host entry wins a tie).
    sort_by_count_desc(&mut candidates, |c| c.embed_count.unwrap_or(0));

    let mut seen = HashSet::new();
    let mut resolved = ResolvedDggStreams::default();
    for candidate in candidates {
        let binding_key = canonical_binding(candidate.platform, &candidate.id);
        if !seen.insert(binding_key.clone()) {
            continue;
        }
        let configured_exact = configured_by_binding.get(&binding_key).copied();
        let configured_alias = identity_aliases
            .get(&binding_key)
            .and_then(|target| configured_by_binding.get(target).copied());
        // YouTube display names are mutable and non-unique: ownership comes
        // only from verified metadata (aliases), never a name guess.
        let configured_name = if candidate.platform == Platform::YouTube {
            None
        } else {
            configured_by_name
                .get(&normalize_id(&candidate.display_name))
                .copied()
        };
        let configured = configured_exact.or(configured_alias).or(configured_name);
        let url = stream_url(candidate.platform, &candidate.id);
        let hosted = candidate.hosting.is_some();
        let mut streamer = Streamer::new(
            format!(
                "dgg:{}:{}",
                candidate.platform,
                omni_core::js::encode_uri_component(&candidate.id.to_lowercase())
            ),
            candidate.display_name.clone(),
            vec![PlatformBinding {
                platform: candidate.platform,
                username: candidate.id.clone(),
                url_override: Some(url.clone()),
            }],
            StreamerTier::Background,
        );
        streamer.discovery_source = Some(DiscoverySource::Dgg);
        streamer.dgg = Some(DggPresence {
            hosted,
            viewers: candidate.embed_count,
        });
        let selected = SelectedDggStream {
            streamer,
            status: FetchedLive {
                title: candidate.title,
                viewer_count: candidate.viewers,
                category: None,
                started_at: candidate.started_at,
            },
            url,
            preview_url: candidate.preview_url,
            embed_count: candidate.embed_count,
            hosted,
            hosting: candidate.hosting,
        };
        if let Some(configured) = configured {
            let previous = resolved.configured_presence.get(&configured.id).copied();
            let viewers = match candidate.embed_count {
                None => previous.and_then(|p| p.viewers),
                Some(count) => Some(previous.and_then(|p| p.viewers).unwrap_or(0) + count),
            };
            resolved.configured_presence.insert(
                configured.id.clone(),
                DggPresence {
                    hosted: previous.is_some_and(|p| p.hosted) || hosted,
                    viewers,
                },
            );
            if configured_exact.is_none() && configured_alias.is_some() {
                resolved
                    .configured_sources
                    .entry(configured.id.clone())
                    .or_default()
                    .push(selected);
            }
            continue;
        }
        if resolved.discovered.len() >= limit {
            continue;
        }
        resolved.discovered.push(selected);
    }
    resolved
}
