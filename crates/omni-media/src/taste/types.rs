//! Taste evidence and profile entities.

use omni_api::media::{
    CommitmentPreferences, MediaType, RecommendationFeedback, RecommendationStatus,
    TasteBehaviorStats, TasteClaim, TasteProfile,
};
use omni_store::cbor::Extra;
use omni_store::entity::Entity;
use serde::{Deserialize, Serialize};

use crate::tmdb::types::TmdbTitleDetails;
use crate::types::WatchedItem;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TasteEvidenceKind {
    #[default]
    PlexWatch,
    RecommendationOutcome,
    ExplicitFeedback,
}

impl TasteEvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TasteEvidenceKind::PlexWatch => "plex_watch",
            TasteEvidenceKind::RecommendationOutcome => "recommendation_outcome",
            TasteEvidenceKind::ExplicitFeedback => "explicit_feedback",
        }
    }
}

/// `recs-taste-evidence`: an append-only observation behind every taste
/// claim. The id is deterministic, so polling the same state is idempotent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasteEvidenceData {
    pub evidence_id: String,
    pub kind: TasteEvidenceKind,
    pub canonical_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    pub media_type: MediaType,
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_minutes: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_countries: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creators: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation_status: Option<RecommendationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<RecommendationFeedback>,
    /// Free-form note left alongside (or instead of) the rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for TasteEvidenceData {
    const NAME: &'static str = "recs-taste-evidence";
    type Key = String;
    fn key(&self) -> String {
        self.evidence_id.clone()
    }
}

/// `recs-taste-profile`: an immutable checkpoint keyed `v<version>:<fingerprint>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "TasteProfileRow", into = "TasteProfileRow")]
pub struct TasteProfileData {
    pub profile: TasteProfile,
    pub extra: Extra,
}

/// The stored shape: profile fields read directly (not through a flattened
/// buffer, which would hide `undefined` from optional fields) plus extras.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TasteProfileRow {
    summary: String,
    stable_preferences: Vec<TasteClaim>,
    conditional_preferences: Vec<TasteClaim>,
    aversions: Vec<TasteClaim>,
    current_saturation: Vec<TasteClaim>,
    exploration_targets: Vec<TasteClaim>,
    uncertainties: Vec<TasteClaim>,
    commitment_preferences: CommitmentPreferences,
    profile_id: String,
    version: i64,
    generated_at: i64,
    evidence_fingerprint: String,
    evidence_count: u64,
    model_id: String,
    prompt_version: String,
    stats: TasteBehaviorStats,
    #[serde(flatten)]
    extra: Extra,
}

impl From<TasteProfileRow> for TasteProfileData {
    fn from(row: TasteProfileRow) -> Self {
        Self {
            profile: TasteProfile {
                summary: row.summary,
                stable_preferences: row.stable_preferences,
                conditional_preferences: row.conditional_preferences,
                aversions: row.aversions,
                current_saturation: row.current_saturation,
                exploration_targets: row.exploration_targets,
                uncertainties: row.uncertainties,
                commitment_preferences: row.commitment_preferences,
                profile_id: row.profile_id,
                version: row.version,
                generated_at: row.generated_at,
                evidence_fingerprint: row.evidence_fingerprint,
                evidence_count: row.evidence_count,
                model_id: row.model_id,
                prompt_version: row.prompt_version,
                stats: row.stats,
            },
            extra: row.extra,
        }
    }
}

impl From<TasteProfileData> for TasteProfileRow {
    fn from(data: TasteProfileData) -> Self {
        let p = data.profile;
        Self {
            summary: p.summary,
            stable_preferences: p.stable_preferences,
            conditional_preferences: p.conditional_preferences,
            aversions: p.aversions,
            current_saturation: p.current_saturation,
            exploration_targets: p.exploration_targets,
            uncertainties: p.uncertainties,
            commitment_preferences: p.commitment_preferences,
            profile_id: p.profile_id,
            version: p.version,
            generated_at: p.generated_at,
            evidence_fingerprint: p.evidence_fingerprint,
            evidence_count: p.evidence_count,
            model_id: p.model_id,
            prompt_version: p.prompt_version,
            stats: p.stats,
            extra: data.extra,
        }
    }
}

impl Entity for TasteProfileData {
    const NAME: &'static str = "recs-taste-profile";
    type Key = String;
    fn key(&self) -> String {
        self.profile.profile_id.clone()
    }
}

/// A resolved completed watch plus its TMDB details, when they loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalWatchObservation {
    pub canonical_id: String,
    pub item: WatchedItem,
    pub metadata: Option<TmdbTitleDetails>,
}
