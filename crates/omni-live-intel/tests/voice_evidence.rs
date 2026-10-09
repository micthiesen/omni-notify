//! Destiny voice evidence accumulation.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use omni_live_intel::voice_evidence::{VoiceEvidenceDecision as D, VoiceEvidenceTracker};

#[test]
fn requires_matches_in_two_independent_samples_before_confirmation() {
    let mut tracker = VoiceEvidenceTracker::new();
    assert_eq!(tracker.observe("guest", 1, 4, 1_000), D::Possible);
    assert_eq!(tracker.observe("guest", 1, 4, 60_000), D::Confirmed);
}

#[test]
fn does_not_combine_matches_more_than_ten_minutes_apart() {
    let mut tracker = VoiceEvidenceTracker::new();
    assert_eq!(tracker.observe("guest", 3, 4, 1_000), D::Possible);
    assert_eq!(tracker.observe("guest", 3, 4, 602_000), D::Possible);
}

#[test]
fn does_not_treat_no_speech_vad_windows_as_negative_evidence() {
    let mut tracker = VoiceEvidenceTracker::new();
    assert_eq!(tracker.observe("guest", 1, 4, 1_000), D::Possible);
    for index in 1..=8 {
        assert_eq!(tracker.observe("guest", 0, 0, index * 60_000), D::None);
    }
    assert_eq!(tracker.observe("guest", 1, 4, 9 * 60_000), D::Confirmed);
}

#[test]
fn forgets_positive_evidence_after_five_speech_negative_scans() {
    let mut tracker = VoiceEvidenceTracker::new();
    assert_eq!(tracker.observe("guest", 1, 4, 1_000), D::Possible);
    for index in 1..=5 {
        assert_eq!(tracker.observe("guest", 0, 4, index * 10_000), D::None);
    }
    assert_eq!(tracker.observe("guest", 1, 4, 60_000), D::Possible);
}

#[test]
fn does_not_count_a_speech_sample_without_a_matching_window() {
    let mut tracker = VoiceEvidenceTracker::new();
    assert_eq!(tracker.observe("guest", 0, 4, 1_000), D::None);
    assert_eq!(tracker.observe("guest", 1, 4, 60_000), D::Possible);
}
