//! Podcast recommendations and the podcast taste profile (owned by WP07;
//! `src/server.ts` podcast serializers and `frontend/src/api.ts`).

use serde::{Deserialize, Serialize, Serializer};

/// A JS number as `JSON.stringify` prints it (integral values without `.0`).
pub fn js_number<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE {
        #[allow(clippy::cast_possible_truncation)]
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

/// [`js_number`] for `Option<f64>`.
pub fn js_number_opt<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => js_number(value, serializer),
        None => serializer.serialize_none(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PodcastRecommendationStatus {
    Pending,
    Notified,
    Listened,
    Abandoned,
    Ignored,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PodcastFeedback {
    GoodPick,
    NotForMe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PodcastQueueResult {
    Queued,
    AlreadyQueued,
    NotQueued,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastShortlistScores {
    #[serde(serialize_with = "js_number")]
    pub taste_match: f64,
    #[serde(serialize_with = "js_number")]
    pub novelty: f64,
    #[serde(serialize_with = "js_number")]
    pub composite: f64,
    pub risks: Vec<String>,
}

/// `serializePodcastRecommendation` (REST).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastRecommendation {
    pub recommendation_id: String,
    pub show_title: String,
    pub episode_title: String,
    pub feed_url: String,
    pub itunes_id: Option<i64>,
    pub artwork_url: Option<String>,
    pub episode_url: Option<String>,
    pub published_at: i64,
    pub duration_minutes: Option<i64>,
    pub status: PodcastRecommendationStatus,
    pub why_for_user: Option<String>,
    pub caveats: Vec<String>,
    #[serde(serialize_with = "js_number_opt")]
    pub confidence: Option<f64>,
    pub shortlist_scores: Option<PodcastShortlistScores>,
    pub discovered_via: Option<String>,
    pub source_url: Option<String>,
    pub matched_voices: Vec<String>,
    pub recommended_at: i64,
    pub notified_at: Option<i64>,
    pub queue_result: Option<PodcastQueueResult>,
    pub feedback: Option<PodcastFeedback>,
    pub feedback_at: Option<i64>,
    pub feedback_note: Option<String>,
}

/// `GET /api/podcast-recommendations`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PodcastRecommendationsResponse {
    pub recommendations: Vec<PodcastRecommendation>,
}

/// `GET /api/podcast-recommendations/:id` and the feedback response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PodcastRecommendationResponse {
    pub recommendation: PodcastRecommendation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastTasteClaim {
    pub claim: String,
    #[serde(serialize_with = "js_number")]
    pub confidence: f64,
    pub evidence_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastRecommendationCounts {
    pub total: u64,
    pub listened: u64,
    pub abandoned: u64,
    pub ignored: u64,
    pub failed: u64,
    pub awaiting_outcome: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastFeedbackCounts {
    pub good_pick: u64,
    pub not_for_me: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastTasteStats {
    pub listened_episodes: u64,
    pub started_episodes: u64,
    pub starred_episodes: u64,
    pub distinct_shows: u64,
    pub recommendations: PodcastRecommendationCounts,
    pub feedback: PodcastFeedbackCounts,
}

/// The stored profile checkpoint (the frontend reads the subset it needs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastTasteProfile {
    pub summary: String,
    pub stable_preferences: Vec<PodcastTasteClaim>,
    pub conditional_preferences: Vec<PodcastTasteClaim>,
    pub aversions: Vec<PodcastTasteClaim>,
    pub current_saturation: Vec<PodcastTasteClaim>,
    pub exploration_targets: Vec<PodcastTasteClaim>,
    pub uncertainties: Vec<PodcastTasteClaim>,
    pub profile_id: String,
    pub version: i64,
    pub generated_at: i64,
    pub evidence_fingerprint: String,
    pub evidence_count: u64,
    pub model_id: String,
    pub prompt_version: String,
    pub stats: PodcastTasteStats,
}

/// `GET /api/podcast-recommendations/taste-profile`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PodcastTasteProfileResponse {
    pub profile: Option<PodcastTasteProfile>,
}

/// `POST /api/podcast-recommendations/:id/feedback` body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PodcastFeedbackRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<PodcastFeedback>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `POST /api/podcast-recommendations/run` body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastRunRequest {
    pub max_recommendations: u32,
}

/// Path builders.
pub mod paths {
    use crate::common::encode_uri_component;

    pub const RECOMMENDATIONS: &str = "/api/podcast-recommendations";
    pub const TASTE_PROFILE: &str = "/api/podcast-recommendations/taste-profile";
    pub const RUN: &str = "/api/podcast-recommendations/run";

    pub fn recommendation(id: &str) -> String {
        format!("{RECOMMENDATIONS}/{}", encode_uri_component(id))
    }

    pub fn feedback(id: &str) -> String {
        format!("{RECOMMENDATIONS}/{}/feedback", encode_uri_component(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommendation_round_trips_with_js_numbers() {
        let json = serde_json::json!({
            "recommendationId": "r1",
            "showTitle": "Show",
            "episodeTitle": "Ep",
            "feedUrl": "https://feeds.example.com/x",
            "itunesId": null,
            "artworkUrl": null,
            "episodeUrl": null,
            "publishedAt": 1_784_091_600_000_i64,
            "durationMinutes": 57,
            "status": "notified",
            "whyForUser": "why",
            "caveats": [],
            "confidence": 1,
            "shortlistScores": { "tasteMatch": 80, "novelty": 60.5, "composite": 59.85, "risks": [] },
            "discoveredVia": null,
            "sourceUrl": null,
            "matchedVoices": [],
            "recommendedAt": 1,
            "notifiedAt": null,
            "queueResult": "already_queued",
            "feedback": "good_pick",
            "feedbackAt": null,
            "feedbackNote": null
        });
        let decoded: PodcastRecommendation = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), json);
    }
}
