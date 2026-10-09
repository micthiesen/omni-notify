//! Media recommendations, the media taste profile and dashboard on-deck items.
//!
//! Stored numbers are `f64` here and serialize as JSON integers when integral
//! (see [`js_number`]), so payloads stay value- and byte-identical to
//! `JSON.stringify` output.

use serde::{Deserialize, Serialize, Serializer};

/// Serializes an `f64` the way `JSON.stringify` prints a JS number: integral
/// values within the safe-integer range without a fraction.
pub fn js_number<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE {
        #[allow(clippy::cast_possible_truncation)]
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

/// [`js_number`] for `Option<f64>` (`None` is `null`).
pub fn js_number_opt<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => js_number(value, serializer),
        None => serializer.serialize_none(),
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum MediaType {
    #[default]
    #[serde(rename = "movie")]
    Movie,
    #[serde(rename = "tv")]
    Tv,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Movie => "movie",
            MediaType::Tv => "tv",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecommendationStatus {
    #[default]
    Pending,
    Notified,
    Watched,
    Abandoned,
    Ignored,
    Failed,
}

impl RecommendationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RecommendationStatus::Pending => "pending",
            RecommendationStatus::Notified => "notified",
            RecommendationStatus::Watched => "watched",
            RecommendationStatus::Abandoned => "abandoned",
            RecommendationStatus::Ignored => "ignored",
            RecommendationStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationFeedback {
    GoodPick,
    NotForMe,
    AlreadyWatched,
}

impl RecommendationFeedback {
    pub fn as_str(self) -> &'static str {
        match self {
            RecommendationFeedback::GoodPick => "good_pick",
            RecommendationFeedback::NotForMe => "not_for_me",
            RecommendationFeedback::AlreadyWatched => "already_watched",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchlistResult {
    Added,
    AlreadyExists,
    Available,
    Error,
}

impl WatchlistResult {
    pub fn as_str(self) -> &'static str {
        match self {
            WatchlistResult::Added => "added",
            WatchlistResult::AlreadyExists => "already_exists",
            WatchlistResult::Available => "available",
            WatchlistResult::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortlistScores {
    #[serde(serialize_with = "js_number")]
    pub taste_match: f64,
    #[serde(serialize_with = "js_number")]
    pub novelty: f64,
    #[serde(serialize_with = "js_number")]
    pub effort_fit: f64,
    #[serde(serialize_with = "js_number")]
    pub composite: f64,
    pub risks: Vec<String>,
}

/// Deep links shown with a recommendation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecommendationLinks {
    pub tmdb: String,
    pub plex: String,
    pub manager: String,
}

/// One media recommendation as the API serves it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub recommendation_id: String,
    pub canonical_id: String,
    #[serde(serialize_with = "js_number")]
    pub tmdb_id: f64,
    pub media_type: MediaType,
    pub title: String,
    #[serde(serialize_with = "js_number_opt")]
    pub year: Option<f64>,
    pub poster_path: Option<String>,
    pub status: RecommendationStatus,
    pub why_for_user: Option<String>,
    pub caveats: Vec<String>,
    pub run_date: String,
    #[serde(serialize_with = "js_number")]
    pub recommended_at: f64,
    #[serde(serialize_with = "js_number_opt")]
    pub notified_at: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    pub started_at: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    pub resolved_at: Option<f64>,
    pub watchlist_result: Option<WatchlistResult>,
    #[serde(serialize_with = "js_number_opt")]
    pub confidence: Option<f64>,
    pub feedback: Option<RecommendationFeedback>,
    #[serde(serialize_with = "js_number_opt")]
    pub feedback_at: Option<f64>,
    pub feedback_note: Option<String>,
    pub source: Option<String>,
    pub genres: Vec<String>,
    #[serde(serialize_with = "js_number_opt")]
    pub runtime_minutes: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    pub season_count: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    pub episode_count: Option<f64>,
    pub series_status: Option<String>,
    pub original_language: Option<String>,
    pub origin_countries: Vec<String>,
    pub creators: Vec<String>,
    pub cast: Vec<String>,
    pub keywords: Vec<String>,
    pub certification: Option<String>,
    pub shortlist_scores: Option<ShortlistScores>,
    pub links: RecommendationLinks,
}

/// `GET /api/recommendations`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecommendationsResponse {
    pub recommendations: Vec<Recommendation>,
}

/// `GET /api/recommendations/:id` and `POST /api/recommendations/:id/feedback`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecommendationResponse {
    pub recommendation: Recommendation,
}

/// `POST /api/recommendations/:id/feedback` body: a rating, a note, or both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<RecommendationFeedback>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `POST /api/recommendations/run` body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    pub max_recommendations: f64,
}

/// `POST /api/recommendations/run` 202 response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResponse {
    pub run_id: String,
}

/// A taste claim with the evidence ids that support it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteClaim {
    pub claim: String,
    #[serde(serialize_with = "js_number")]
    pub confidence: f64,
    pub evidence_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitmentPreference {
    Positive,
    Neutral,
    Negative,
    Uncertain,
}

impl CommitmentPreference {
    pub fn as_str(self) -> &'static str {
        match self {
            CommitmentPreference::Positive => "positive",
            CommitmentPreference::Neutral => "neutral",
            CommitmentPreference::Negative => "negative",
            CommitmentPreference::Uncertain => "uncertain",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitmentAssessment {
    pub preference: CommitmentPreference,
    #[serde(serialize_with = "js_number")]
    pub confidence: f64,
    pub evidence_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitmentPreferences {
    pub movies: CommitmentAssessment,
    pub limited_series: CommitmentAssessment,
    pub long_series: CommitmentAssessment,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationOutcomeStats {
    pub total: u64,
    pub watched: u64,
    pub abandoned: u64,
    pub ignored: u64,
    pub failed: u64,
    pub awaiting_outcome: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackStats {
    pub good_pick: u64,
    pub not_for_me: u64,
    pub already_watched: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePerformance {
    pub total: u64,
    pub watched: u64,
    pub good_pick: u64,
    pub not_for_me: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteBehaviorStats {
    pub completed_movies: u64,
    pub completed_series: u64,
    pub rewatched_titles: u64,
    pub recommendations: RecommendationOutcomeStats,
    pub feedback: FeedbackStats,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_number_opt"
    )]
    pub average_hours_to_start: Option<f64>,
    /// Keyed by candidate source, in JS object insertion order.
    pub source_performance: SourcePerformanceMap,
}

/// Source name to performance, in JS insertion order.
pub type SourcePerformanceMap = OrderedMap<SourcePerformance>;

/// A string-keyed map that keeps insertion order (a JS object).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderedMap<V>(pub Vec<(String, V)>);

impl<V> OrderedMap<V> {
    pub fn get(&self, key: &str) -> Option<&V> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Inserts at the end, or replaces in place.
    pub fn insert(&mut self, key: String, value: V) {
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for OrderedMap<V> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for OrderedVisitor<V> {
            type Value = OrderedMap<V>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut out = OrderedMap(Vec::new());
                while let Some((key, value)) = access.next_entry::<String, V>()? {
                    out.insert(key, value);
                }
                Ok(out)
            }
        }
        deserializer.deserialize_map(OrderedVisitor(std::marker::PhantomData))
    }
}

/// `TasteProfileData` as persisted and served (stored field order).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteProfile {
    pub summary: String,
    pub stable_preferences: Vec<TasteClaim>,
    pub conditional_preferences: Vec<TasteClaim>,
    pub aversions: Vec<TasteClaim>,
    pub current_saturation: Vec<TasteClaim>,
    pub exploration_targets: Vec<TasteClaim>,
    pub uncertainties: Vec<TasteClaim>,
    pub commitment_preferences: CommitmentPreferences,
    pub profile_id: String,
    pub version: i64,
    pub generated_at: i64,
    pub evidence_fingerprint: String,
    pub evidence_count: u64,
    pub model_id: String,
    pub prompt_version: String,
    pub stats: TasteBehaviorStats,
}

/// `GET /api/recommendations/taste-profile`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TasteProfileResponse {
    pub profile: Option<TasteProfile>,
}

/// One dashboard "On Deck" entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnDeckItem {
    pub recommendation_id: String,
    pub title: String,
    pub media_type: MediaType,
    #[serde(serialize_with = "js_number_opt")]
    pub year: Option<f64>,
    pub poster_path: Option<String>,
    pub why_for_user: Option<String>,
    #[serde(serialize_with = "js_number")]
    pub recommended_at: f64,
}

/// Path builders for the media routes (the frontend uses these).
pub mod paths {
    use crate::common::encode_uri_component;

    pub const RECOMMENDATIONS: &str = "/api/recommendations";
    pub const TASTE_PROFILE: &str = "/api/recommendations/taste-profile";
    pub const RUN: &str = "/api/recommendations/run";

    /// `GET /api/recommendations/:id`.
    pub fn recommendation(id: &str) -> String {
        format!("{RECOMMENDATIONS}/{}", encode_uri_component(id))
    }

    /// `POST /api/recommendations/:id/feedback`.
    pub fn feedback(id: &str) -> String {
        format!("{RECOMMENDATIONS}/{}/feedback", encode_uri_component(id))
    }
}
