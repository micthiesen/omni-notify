//! Streamers: aggregate identities over platform bindings.

use std::sync::{Arc, RwLock};

use omni_api::streamers::{DggPresence, StreamerTier};

use crate::platform::{Platform, PlatformBinding};
/// JS `String.prototype.trim` (`omni_core::js::trim`).
pub use omni_core::js::trim as js_trim;

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
    /// Never set for configured streamers.
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

/// `displayName.trim().toLowerCase()`.
pub fn normalize_id(display_name: &str) -> String {
    js_trim(display_name).to_lowercase()
}

/// Whether a streamer is polled on this tick; tick 0 (startup) polls everyone.
pub fn is_streamer_due(tier: StreamerTier, tick: u64) -> bool {
    tier != StreamerTier::Background || tick.is_multiple_of(BACKGROUND_POLL_FACTOR)
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
