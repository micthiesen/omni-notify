//! Rolling summary text formatting.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use omni_live_intel::summary_text::{
    are_same_livestream_topic, clean_livestream_summary, clean_livestream_topic,
};

#[test]
fn keeps_complete_concise_summaries_unchanged() {
    let summary = "They compare two melanoma treatments. The discussion turns to side effects.";
    assert_eq!(clean_livestream_summary(summary), summary);
}

#[test]
fn drops_constrained_output_artifacts_and_keeps_the_last_complete_sentence() {
    assert_eq!(
        clean_livestream_summary(
            "The speaker explains how immunotherapy works. The treatment can cause severe side effects. Because melanoma can depend on免"
        ),
        "The speaker explains how immunotherapy works. The treatment can cause severe side effects."
    );
}

#[test]
fn uses_an_ellipsis_when_the_model_returns_only_an_unfinished_sentence() {
    assert_eq!(
        clean_livestream_summary(
            "The discussion compares personalized cancer vaccines with a small pancreatic cancer trial that had noক"
        ),
        "The discussion compares personalized cancer vaccines with a small pancreatic cancer trial that had…"
    );
}

#[test]
fn falls_back_safely_when_the_response_is_only_an_artifact() {
    assert_eq!(
        clean_livestream_summary("####"),
        "The current discussion could not be summarized cleanly."
    );
    assert_eq!(clean_livestream_topic("####"), "Current discussion");
}

#[test]
fn removes_artifact_tails_and_respects_the_compact_label_limit() {
    assert_eq!(
        clean_livestream_topic("Personalized mRNA melanoma immunotherapy####"),
        "Personalized mRNA melanoma immunotherapy"
    );
    assert_eq!(
        clean_livestream_topic(
            "Debate over Professor Dave's claims about the Al-Ahli hospital explosion and its aftermath"
        ),
        "Debate over Professor Dave's claims about the Al-Ahli…"
    );
}

#[test]
fn merges_labels_with_substantial_subject_overlap() {
    assert!(are_same_livestream_topic(
        "Melanoma immunotherapy benefits and risks",
        "Benefits and side effects of melanoma immunotherapy"
    ));
}

#[test]
fn keeps_genuinely_different_subjects_separate() {
    assert!(!are_same_livestream_topic(
        "Melanoma immunotherapy benefits and risks",
        "Gaming performance and matchmaking jokes"
    ));
}
