//! Canonical show/episode identities and the candidate shapes that flow
//! through the pipeline.

use std::sync::LazyLock;

use regex::Regex;

/// `itunes:{collectionId}` when Apple knows the show, else `feed:{normalized feed URL}`.
pub type CanonicalShowId = String;
/// `{showId}#{rss item guid}`.
pub type CanonicalEpisodeId = String;

static PROTOCOL: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?i)^https?://").ok());

/// `makeShowId`: a truthy iTunes id wins, then a non-empty feed URL.
pub fn make_show_id(itunes_id: Option<i64>, feed_url: Option<&str>) -> Option<CanonicalShowId> {
    if let Some(id) = itunes_id.filter(|id| *id != 0) {
        return Some(format!("itunes:{id}"));
    }
    feed_url
        .filter(|url| !url.is_empty())
        .map(|url| format!("feed:{}", normalize_feed_url(url)))
}

/// `makeEpisodeId`.
pub fn make_episode_id(show_id: &str, episode_guid: &str) -> CanonicalEpisodeId {
    format!("{show_id}#{episode_guid}")
}

/// Strips the protocol, trailing slashes and case.
pub fn normalize_feed_url(feed_url: &str) -> String {
    let trimmed = feed_url.trim();
    let without_protocol = match PROTOCOL.as_ref() {
        Some(re) => re.replace(trimmed, "").into_owned(),
        None => trimmed.to_owned(),
    };
    without_protocol.trim_end_matches('/').to_lowercase()
}

/// An episode surfaced by discovery, before resolution against iTunes/RSS.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiscoveredEpisode {
    pub show_title: String,
    pub episode_title: String,
    /// One line on why discovery surfaced it.
    pub context: String,
    pub source_url: Option<String>,
    /// Followed voices this episode features as guests, when guest-driven.
    pub matched_voices: Option<Vec<String>>,
}

/// A fully resolved candidate: identity, verified release date, metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EpisodeCandidate {
    pub episode_id: CanonicalEpisodeId,
    pub show_id: CanonicalShowId,
    pub show_title: String,
    pub episode_title: String,
    pub feed_url: String,
    pub itunes_id: Option<i64>,
    pub artwork_url: Option<String>,
    pub episode_guid: String,
    /// Enclosure URL, the reliable key for matching against Castro.
    pub media_url: Option<String>,
    pub episode_url: Option<String>,
    /// Verified from the show's RSS feed, never from search snippets.
    pub published_at: i64,
    pub duration_minutes: Option<i64>,
    pub description: String,
    pub show_genres: Vec<String>,
    pub discovered_via: String,
    pub source_url: Option<String>,
    /// Followed voices appearing as guests (Tier-1 candidates).
    pub matched_voices: Option<Vec<String>>,
}
