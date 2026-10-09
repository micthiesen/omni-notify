//! Port of `src/live-check/intelligence/presencePolicy.spec.ts`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use omni_live_intel::presence_policy::{VoiceMatchAction, decide_voice_match_action};
use omni_live_intel::types::{DestinyPresence, PresenceState};
use omni_live_intel::voice_evidence::VoiceEvidenceDecision;

fn confirmed() -> DestinyPresence {
    DestinyPresence {
        state: PresenceState::Confirmed,
        confidence: 0.71,
        detected_at: 1_000,
        reason: "Live conversation confirmed".into(),
        extra: Default::default(),
    }
}

#[test]
fn never_downgrades_a_current_confirmation_to_possible() {
    assert_eq!(
        decide_voice_match_action(VoiceEvidenceDecision::Possible, Some(&confirmed())),
        VoiceMatchAction::RetainConfirmed
    );
}

#[test]
fn reuses_a_current_confirmation_instead_of_paying_to_verify_it_again() {
    assert_eq!(
        decide_voice_match_action(VoiceEvidenceDecision::Confirmed, Some(&confirmed())),
        VoiceMatchAction::RetainConfirmed
    );
}

#[test]
fn still_requires_verification_for_newly_confirmed_repeated_evidence() {
    assert_eq!(
        decide_voice_match_action(VoiceEvidenceDecision::Confirmed, None),
        VoiceMatchAction::Verify
    );
}
