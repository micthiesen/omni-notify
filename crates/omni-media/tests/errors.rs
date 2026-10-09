//! Recommendation error wrapping and cause messages.
#![allow(clippy::expect_used)]

use omni_media::error::{IntegrationError, RecommendationError, cause_message};

#[test]
fn preserves_the_integration_operation_in_the_typed_error() {
    let error = IntegrationError::new("TMDB lookup", "offline");
    assert!(error.to_string().contains("TMDB lookup failed: offline"));
    assert_eq!(error.cause_message(), "offline");
}

#[test]
fn uses_a_tagged_integration_error() {
    let error: RecommendationError = IntegrationError::new("Plex history", "timed out").into();
    assert!(matches!(
        &error,
        RecommendationError::Integration(IntegrationError { operation, .. }) if operation == "Plex history"
    ));
    assert_eq!(cause_message(&error), "timed out");
}
