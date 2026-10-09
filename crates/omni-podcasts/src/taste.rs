//! Combined taste evidence for prompts: the hand-written seed
//! profile, subscribed shows, explicit feedback, and the latest reflective
//! profile.

use std::path::Path;

use omni_store::{Store, StoreError};

use crate::persistence::{format_podcast_feedback_digest_from, get_all_podcast_recommendations};
use crate::reflection::format_podcast_taste_profile_digest;
use crate::reflection::store::get_latest_podcast_taste_profile;
use crate::subscriptions::{SubscriptionState, format_subscriptions_digest};

pub async fn build_taste_digest(
    store: &Store,
    subscriptions: &SubscriptionState,
    seed: &str,
) -> Result<String, StoreError> {
    let feedback =
        format_podcast_feedback_digest_from(&get_all_podcast_recommendations(store).await?);
    let profile = get_latest_podcast_taste_profile(store).await?;
    Ok([
        seed.to_owned(),
        format_subscriptions_digest(subscriptions),
        feedback,
        format_podcast_taste_profile_digest(profile.as_ref()),
    ]
    .into_iter()
    .filter(|section| !section.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TasteSeedFailure {
    Unreadable,
    Malformed,
}

/// `TasteSeedError`.
#[derive(Debug, thiserror::Error)]
pub struct TasteSeedError {
    pub path: String,
    pub reason: TasteSeedFailure,
    pub cause: Option<std::io::Error>,
}

impl std::fmt::Display for TasteSeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            TasteSeedFailure::Malformed => {
                write!(f, "Podcast taste seed at {} is empty", self.path)
            }
            TasteSeedFailure::Unreadable => {
                write!(f, "Could not read podcast taste seed at {}", self.path)?;
                if let Some(cause) = &self.cause {
                    write!(f, ": {cause}")?;
                }
                Ok(())
            }
        }
    }
}

/// Loads the configured seed (`""` when no path is configured); an
/// unreadable or blank file is a typed failure.
pub async fn load_taste_seed(path: Option<&str>) -> Result<String, TasteSeedError> {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return Ok(String::new());
    };
    let contents = tokio::fs::read_to_string(Path::new(path))
        .await
        .map_err(|cause| TasteSeedError {
            path: path.to_owned(),
            reason: TasteSeedFailure::Unreadable,
            cause: Some(cause),
        })?;
    check_seed(path, &contents)
}

/// Trims and rejects an empty seed.
pub fn check_seed(path: &str, contents: &str) -> Result<String, TasteSeedError> {
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Err(TasteSeedError {
            path: path.to_owned(),
            reason: TasteSeedFailure::Malformed,
            cause: None,
        });
    }
    Ok(trimmed.to_owned())
}

/// `None` when `path` is a readable regular file, else the reason.
pub fn describe_unreadable_file(path: &str) -> Option<String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => None,
        Ok(_) => Some("is not a file (expected a markdown file, not a directory)".to_owned()),
        Err(error) => Some(format!("could not be read: {error}")),
    }
}
