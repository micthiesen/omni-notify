//! Media taste evidence, profile reflection and stats (`src/recommendations/taste/`).

pub mod evidence;
pub mod persistence;
pub mod reflection;
pub mod stats;
pub mod task;
pub mod types;

pub use evidence::{derive_recommendation_evidence, derive_watch_evidence, fingerprint_evidence};
pub use persistence::{
    get_all_taste_evidence, get_latest_taste_profile, insert_taste_evidence, insert_taste_profile,
};
pub use reflection::{
    TASTE_PROMPT_VERSION, format_taste_profile_digest, run_taste_reflection,
    select_reflection_evidence, validate_profile,
};
pub use stats::compute_behavioral_stats;
pub use types::{
    CanonicalWatchObservation, TasteEvidenceData, TasteEvidenceKind, TasteProfileData,
};
