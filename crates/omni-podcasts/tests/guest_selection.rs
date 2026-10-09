//! Guest selection.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use omni_podcasts::guest_selection::{GuestDecision, apply_guest_decisions};
use omni_podcasts::selection::NotificationCopy;
use omni_podcasts::types::EpisodeCandidate;

fn candidate(id: &str) -> EpisodeCandidate {
    EpisodeCandidate {
        episode_id: id.into(),
        show_id: "itunes:1".into(),
        show_title: format!("Show {id}"),
        episode_title: format!("Ep {id}"),
        feed_url: "https://f".into(),
        episode_guid: "g".into(),
        published_at: 0,
        discovered_via: "guest".into(),
        ..EpisodeCandidate::default()
    }
}

fn decision(id: &str) -> GuestDecision {
    GuestDecision {
        candidate_id: id.into(),
        include: true,
        reason: String::new(),
        why_for_user: Some("why".into()),
        caveats: Vec::new(),
        confidence: 0.5,
        notification: Some(NotificationCopy {
            title: "t".into(),
            message: "m".into(),
        }),
    }
}

fn by_id(ids: &[&str]) -> HashMap<String, EpisodeCandidate> {
    ids.iter()
        .map(|id| ((*id).to_owned(), candidate(id)))
        .collect()
}

fn ids(picks: &[omni_podcasts::guest_selection::GuestPick]) -> Vec<String> {
    picks
        .iter()
        .map(|p| p.candidate.episode_id.clone())
        .collect()
}

#[test]
fn keeps_includes_and_drops_excludes() {
    let picks = apply_guest_decisions(
        &[
            decision("e1"),
            GuestDecision {
                include: false,
                ..decision("e2")
            },
        ],
        &by_id(&["e1", "e2"]),
        5,
    );
    assert_eq!(ids(&picks), vec!["e1"]);
}

#[test]
fn dedups_a_repeated_candidate_id_no_double_commit() {
    let picks = apply_guest_decisions(&[decision("e1"), decision("e1")], &by_id(&["e1"]), 5);
    assert_eq!(picks.len(), 1);
}

#[test]
fn keeps_the_strongest_when_over_max_regardless_of_array_order() {
    let picks = apply_guest_decisions(
        &[
            GuestDecision {
                confidence: 0.2,
                ..decision("e1")
            },
            GuestDecision {
                confidence: 0.9,
                ..decision("e2")
            },
            GuestDecision {
                confidence: 0.5,
                ..decision("e3")
            },
        ],
        &by_id(&["e1", "e2", "e3"]),
        2,
    );
    assert_eq!(ids(&picks), vec!["e2", "e3"]);
}

#[test]
fn skips_unknown_candidate_ids() {
    let picks = apply_guest_decisions(&[decision("nope")], &by_id(&["e1"]), 5);
    assert!(picks.is_empty());
}

#[test]
fn skips_includes_missing_why_for_user_or_notification() {
    let picks = apply_guest_decisions(
        &[
            GuestDecision {
                why_for_user: None,
                ..decision("e1")
            },
            GuestDecision {
                notification: None,
                ..decision("e2")
            },
        ],
        &by_id(&["e1", "e2"]),
        5,
    );
    assert!(picks.is_empty());
}
