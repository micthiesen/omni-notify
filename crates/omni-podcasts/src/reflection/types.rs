//! Taste evidence and profile entities.

use omni_store::cbor::Extra;
use omni_store::entity::Entity;
use serde::{Deserialize, Serialize};

use crate::persistence::{PodcastFeedback, PodcastRecommendationStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PodcastTasteEvidenceKind {
    /// A playback event from listen history.
    Listen,
    /// The passive outcome of a delivered recommendation.
    RecommendationOutcome,
    /// Explicit good-pick/not-for-me feedback (or a note).
    ExplicitFeedback,
}

/// `podcast-taste-evidence`: append-only observation; the show is the
/// independence unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastTasteEvidenceData {
    pub evidence_id: String,
    pub kind: PodcastTasteEvidenceKind,
    /// Normalized show title used to count independent shows per claim.
    pub show_key: String,
    pub show_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_via: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_voices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_minutes: Option<i64>,
    pub observed_at: i64,
    /// 0-1 fraction listened, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starred: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation_status: Option<PodcastRecommendationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<PodcastFeedback>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PodcastTasteEvidenceData {
    const NAME: &'static str = "podcast-taste-evidence";
    type Key = String;
    fn key(&self) -> String {
        self.evidence_id.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastTasteClaim {
    pub claim: String,
    pub confidence: f64,
    pub evidence_ids: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationCounts {
    pub total: u64,
    pub listened: u64,
    pub abandoned: u64,
    pub ignored: u64,
    pub failed: u64,
    pub awaiting_outcome: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackCounts {
    pub good_pick: u64,
    pub not_for_me: u64,
}

/// Deterministic counts injected into the prompt as ground truth.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastBehavioralStats {
    /// Finished (>=80%, or a playback event with no completion data).
    pub listened_episodes: u64,
    /// Any playback activity.
    pub started_episodes: u64,
    pub starred_episodes: u64,
    pub distinct_shows: u64,
    pub recommendations: RecommendationCounts,
    pub feedback: FeedbackCounts,
}

/// `podcast-taste-profile`: an immutable checkpoint keyed
/// `v{version}:{evidenceFingerprint}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodcastTasteProfileData {
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
    pub stats: PodcastBehavioralStats,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PodcastTasteProfileData {
    const NAME: &'static str = "podcast-taste-profile";
    type Key = String;
    fn key(&self) -> String {
        self.profile_id.clone()
    }
}

/// The claim lists of a profile (before checkpoint metadata).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PodcastTasteProfileContent {
    pub summary: String,
    pub stable_preferences: Vec<PodcastTasteClaim>,
    pub conditional_preferences: Vec<PodcastTasteClaim>,
    pub aversions: Vec<PodcastTasteClaim>,
    pub current_saturation: Vec<PodcastTasteClaim>,
    pub exploration_targets: Vec<PodcastTasteClaim>,
    pub uncertainties: Vec<PodcastTasteClaim>,
}
