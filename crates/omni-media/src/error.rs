//! Typed recommendation failures.

use omni_store::StoreError;

/// A failed integration or model call: `"<operation> failed: <cause>"`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{operation} failed: {cause}")]
pub struct IntegrationError {
    pub operation: String,
    /// The innermost cause message (`effectMessage`).
    pub cause: String,
}

impl IntegrationError {
    pub fn new(operation: impl Into<String>, cause: impl std::fmt::Display) -> Self {
        Self {
            operation: operation.into(),
            cause: cause.to_string(),
        }
    }

    /// Wraps an error by its innermost cause (`toolErrorMessage` rule).
    pub fn from_error(operation: impl Into<String>, error: &dyn std::error::Error) -> Self {
        Self {
            operation: operation.into(),
            cause: omni_core::error::chain_message(error),
        }
    }

    /// `effectMessage`: the cause message without the operation prefix.
    pub fn effect_message(&self) -> &str {
        &self.cause
    }
}

/// Every failure of the recommendation pipeline and taste reflection.
#[derive(Debug, thiserror::Error)]
pub enum RecommendationError {
    #[error(transparent)]
    Integration(#[from] IntegrationError),
    /// `RecommendationInputError`.
    #[error("{0}")]
    Input(String),
    /// `RecommendationPersistenceError`.
    #[error("{operation} failed: {source}")]
    Persistence {
        operation: &'static str,
        #[source]
        source: StoreError,
    },
    /// `RecommendationCommitError`.
    #[error("{0}")]
    Commit(String),
    /// `TasteReflectionOutputError`.
    #[error("{0}")]
    TasteReflectionOutput(String),
}

impl RecommendationError {
    pub fn persistence(operation: &'static str) -> impl FnOnce(StoreError) -> Self {
        move |source| RecommendationError::Persistence { operation, source }
    }
}

/// `effectMessage` for any error: integration errors unwrap to their cause.
pub fn effect_message(error: &RecommendationError) -> String {
    match error {
        RecommendationError::Integration(e) => e.cause.clone(),
        other => other.to_string(),
    }
}

/// The message for an invalid `maxRecommendations`.
pub fn max_recommendations_message(max: u32) -> String {
    format!("maxRecommendations must be an integer from 1 to {max}")
}
