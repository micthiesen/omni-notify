//! Shared recommendation types.

use serde::{Deserialize, Serialize};

pub use omni_api::media::MediaType;

/// `tmdb:<mediaType>:<tmdbId>`, the canonical content identity.
pub fn make_canonical_id(media_type: MediaType, tmdb_id: i64) -> String {
    format!("tmdb:{}:{tmdb_id}", media_type.as_str())
}

/// The TMDB id of a canonical id (`Number(canonicalId.split(":")[2])`).
pub fn canonical_tmdb_id(canonical_id: &str) -> Option<i64> {
    canonical_id.split(':').nth(2)?.parse().ok()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb: Option<i64>,
}

impl ExternalIds {
    pub fn is_empty(&self) -> bool {
        self.tmdb.is_none() && self.imdb.is_none() && self.tvdb.is_none()
    }

    /// `{ ...self, ...other }` where `other`'s present fields win.
    pub fn overlay(&self, other: Option<&ExternalIds>) -> ExternalIds {
        let mut merged = self.clone();
        if let Some(other) = other {
            if other.tmdb.is_some() {
                merged.tmdb = other.tmdb;
            }
            if other.imdb.is_some() {
                merged.imdb.clone_from(&other.imdb);
            }
            if other.tvdb.is_some() {
                merged.tvdb = other.tvdb;
            }
        }
        merged
    }
}

/// An item as known to the local media library or watchlist service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    /// Opaque server-native id (a Plex GUID, `radarr:<id>`, `sonarr:<id>`).
    pub guid: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    pub media_type: MediaType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ids: Option<ExternalIds>,
    /// Sonarr's generated URL slug, when the backend exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_slug: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchedItem {
    #[serde(flatten)]
    pub item: MediaItem,
    /// Epoch ms.
    pub viewed_at: i64,
    pub view_count: i64,
    /// 0-1 fraction of runtime watched, when the backend reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InProgressItem {
    #[serde(flatten)]
    pub item: MediaItem,
    /// 0-1 fraction of runtime watched.
    pub progress: f64,
    pub last_viewed_at: i64,
}

/// Three-state service result: unavailable is never empty state.
#[derive(Clone, Debug, PartialEq)]
pub enum FetchResult<T> {
    Ok(T),
    Unavailable { reason: String },
}

impl<T> FetchResult<T> {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        FetchResult::Unavailable {
            reason: reason.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddToWatchlistResult {
    Added,
    AlreadyExists,
    NotFound,
    Unavailable,
    Error,
}

impl AddToWatchlistResult {
    pub fn as_str(self) -> &'static str {
        match self {
            AddToWatchlistResult::Added => "added",
            AddToWatchlistResult::AlreadyExists => "already_exists",
            AddToWatchlistResult::NotFound => "not_found",
            AddToWatchlistResult::Unavailable => "unavailable",
            AddToWatchlistResult::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchlistAddOutcome {
    pub result: AddToWatchlistResult,
    /// Sonarr's generated series slug, when known (UI deep links).
    pub title_slug: Option<String>,
}

impl WatchlistAddOutcome {
    pub fn of(result: AddToWatchlistResult) -> Self {
        Self {
            result,
            title_slug: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CandidateSource {
    Similar,
    Discover,
    #[default]
    Trending,
    Novelty,
}

impl CandidateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateSource::Similar => "similar",
            CandidateSource::Discover => "discover",
            CandidateSource::Trending => "trending",
            CandidateSource::Novelty => "novelty",
        }
    }
}

/// A candidate title assembled from TMDB, pre-scoring.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    pub canonical_id: String,
    pub tmdb_id: i64,
    pub media_type: MediaType,
    pub title: String,
    pub year: Option<i64>,
    pub overview: String,
    pub genres: Vec<String>,
    pub vote_average: f64,
    pub vote_count: f64,
    pub popularity: f64,
    pub poster_path: Option<String>,
    pub runtime_minutes: Option<f64>,
    pub season_count: Option<f64>,
    pub episode_count: Option<f64>,
    pub series_status: Option<String>,
    pub original_language: Option<String>,
    pub origin_countries: Option<Vec<String>>,
    pub creators: Option<Vec<String>>,
    pub cast: Option<Vec<String>>,
    pub keywords: Option<Vec<String>>,
    pub certification: Option<String>,
    pub source: CandidateSource,
    /// Present in the local media library (a positive signal, not an exclusion).
    pub in_library: bool,
}
