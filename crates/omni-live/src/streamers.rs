//! Streamers: aggregate identities over platform bindings (`streamers.ts`).

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use omni_api::streamers::{DggPresence, StreamerTier};

use crate::channels::ChannelsConfig;
use crate::platform::{Platform, PlatformBinding};

/// Background streamers are polled every Nth tick (20 s base, 60 s effective).
pub const BACKGROUND_POLL_FACTOR: u64 = 3;

/// Transient discovery provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoverySource {
    Dgg,
}

/// One tracked streamer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Streamer {
    pub id: String,
    pub display_name: String,
    pub bindings: Vec<PlatformBinding>,
    pub pushover_token: Option<String>,
    /// `Some(false)` mutes live, offline and title notifications.
    pub live_notifications: Option<bool>,
    pub tier: StreamerTier,
    /// Current Destiny.gg placement metadata.
    pub dgg: Option<DggPresence>,
    /// Never set for channels from `channels.json`.
    pub discovery_source: Option<DiscoverySource>,
}

impl Streamer {
    /// A primary-tier streamer with default overrides (tests and DGG).
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        bindings: Vec<PlatformBinding>,
        tier: StreamerTier,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            bindings,
            pushover_token: None,
            live_notifications: None,
            tier,
            dgg: None,
            discovery_source: None,
        }
    }

    /// Rank-only viewer count: DGG-only discoveries compete by DGG audience.
    pub fn ordering_viewer_count(&self) -> Option<i64> {
        self.discovery_source
            .map(|DiscoverySource::Dgg| self.dgg.and_then(|d| d.viewers).unwrap_or(0))
    }
}

/// JS `String.prototype.trim`: strips ECMAScript WhiteSpace and
/// LineTerminator characters (unlike `str::trim`, it removes U+FEFF and keeps
/// U+0085).
pub fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// `displayName.trim().toLowerCase()`.
pub fn normalize_id(display_name: &str) -> String {
    js_trim(display_name).to_lowercase()
}

/// Whether a streamer is polled on this tick; tick 0 (startup) polls everyone.
pub fn is_streamer_due(tier: StreamerTier, tick: u64) -> bool {
    tier != StreamerTier::Background || tick.is_multiple_of(BACKGROUND_POLL_FACTOR)
}

/// A config error found while building streamers (cross-entry checks).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BuildStreamersError {
    #[error(
        "Duplicate streamer \"{display_name}\" in channels config (normalizes to \"{id}\", already used by another entry)"
    )]
    DuplicateStreamer { display_name: String, id: String },
    #[error("Duplicate platform binding \"{0}\" across channel entries")]
    DuplicateBinding(String),
}

/// One streamer per entry; bindings always ordered youtube, twitch, kick.
pub fn build_streamers(config: &ChannelsConfig) -> Result<Vec<Streamer>, BuildStreamersError> {
    let mut streamers: Vec<Streamer> = Vec::with_capacity(config.len());
    let mut seen_ids = HashSet::new();
    let mut seen_bindings = HashSet::new();
    for (display_name, entry) in config {
        let id = normalize_id(display_name);
        if !seen_ids.insert(id.clone()) {
            return Err(BuildStreamersError::DuplicateStreamer {
                display_name: display_name.clone(),
                id,
            });
        }
        let mut bindings = Vec::new();
        for (platform, usernames) in [
            (Platform::YouTube, &entry.youtube),
            (Platform::Twitch, &entry.twitch),
            (Platform::Kick, &entry.kick),
        ] {
            for username in usernames.iter().flat_map(|u| u.to_vec()) {
                let key = format!("{platform}:{username}");
                if !seen_bindings.insert(key.clone()) {
                    return Err(BuildStreamersError::DuplicateBinding(key));
                }
                bindings.push(PlatformBinding::new(platform, username));
            }
        }
        streamers.push(Streamer {
            id,
            display_name: display_name.clone(),
            bindings,
            pushover_token: entry.pushover_token.clone(),
            live_notifications: entry.live_notifications,
            tier: entry.tier.unwrap_or_default(),
            dgg: None,
            discovery_source: None,
        });
    }
    Ok(streamers)
}

/// Drops every binding of `platform`, removing streamers left with none.
/// Returns whether anything was dropped.
pub fn drop_platform_bindings(
    streamers: Vec<Streamer>,
    platform: Platform,
) -> (Vec<Streamer>, bool) {
    let mut dropped_any = false;
    let result = streamers
        .into_iter()
        .map(|mut streamer| {
            if streamer.bindings.iter().any(|b| b.platform == platform) {
                dropped_any = true;
                streamer.bindings.retain(|b| b.platform != platform);
            }
            streamer
        })
        .filter(|streamer| !streamer.bindings.is_empty())
        .collect();
    (result, dropped_any)
}

/// The shared, ordered streamer list: configured streamers (DGG-enriched)
/// followed by transient DGG discoveries. The live check replaces it each DGG
/// refresh; routes, iOS slots and the `LiveDirectory` read snapshots.
#[derive(Clone, Debug, Default)]
pub struct Roster {
    inner: Arc<RwLock<Vec<Streamer>>>,
}

impl Roster {
    pub fn new(streamers: Vec<Streamer>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(streamers)),
        }
    }

    /// The current list (cloned; never held across awaits).
    pub fn snapshot(&self) -> Vec<Streamer> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn get(&self, id: &str) -> Option<Streamer> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|s| s.id == id)
    }

    pub fn len(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn replace(&self, streamers: Vec<Streamer>) {
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = streamers;
    }
}
