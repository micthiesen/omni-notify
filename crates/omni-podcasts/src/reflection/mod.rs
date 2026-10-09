//! Weekly podcast taste reflection from listen history and recommendation
//! outcomes.

pub mod core;
pub mod evidence;
pub mod stats;
pub mod store;
pub mod task;
pub mod types;

pub use self::core::{
    PODCAST_TASTE_PROMPT_VERSION, format_podcast_taste_profile_digest,
    run_podcast_taste_reflection, select_podcast_reflection_evidence, validate_podcast_profile,
};
pub use evidence::{
    derive_listen_evidence, derive_recommendation_evidence, fingerprint_evidence,
    normalize_show_key,
};
pub use stats::compute_podcast_behavioral_stats;
pub use types::{PodcastTasteEvidenceData, PodcastTasteProfileData};
