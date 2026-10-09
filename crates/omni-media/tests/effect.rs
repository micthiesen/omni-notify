//! Port of `src/recommendations/effect.spec.ts`.
//!
//! Dropped case: "accepts synchronous third-party test doubles" exercises the
//! Promise adapter `integrationEffect`; Rust has no Promise boundary, so a
//! synchronous double is an ordinary function call with nothing to adapt.
#![allow(clippy::expect_used)]

use omni_media::error::{IntegrationError, RecommendationError, effect_message};

#[test]
fn preserves_the_integration_operation_in_the_typed_error() {
    let error = IntegrationError::new("TMDB lookup", "offline");
    assert!(error.to_string().contains("TMDB lookup failed: offline"));
    assert_eq!(error.effect_message(), "offline");
}

#[test]
fn uses_a_tagged_integration_error() {
    let error: RecommendationError = IntegrationError::new("Plex history", "timed out").into();
    assert!(matches!(
        &error,
        RecommendationError::Integration(IntegrationError { operation, .. }) if operation == "Plex history"
    ));
    assert_eq!(effect_message(&error), "timed out");
}
