//! Port of `src/live-check/intelligence/alertPolicy.spec.ts`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use indexmap::IndexMap;
use omni_live_intel::alert_policy::{alert_sent_in_session, livestream_alert_confidence_floor};
use omni_live_intel::types::{
    LivestreamAlertRecord, LivestreamAlertType, LivestreamIntelligenceData,
};

fn state() -> LivestreamIntelligenceData {
    LivestreamIntelligenceData {
        streamer_id: "darius".into(),
        session_started_at: 1_000,
        relevance_score: 0.0,
        relevance_reasons: vec![],
        chapters: vec![],
        updated_at: 2_000,
        semantic: None,
        trend: None,
        summary: None,
        destiny_presence: None,
        latest_alert: None,
        alerted_at_by_type: None,
        extra: Default::default(),
    }
}

fn alert(id: &str, alert_type: LivestreamAlertType, created_at: i64) -> LivestreamAlertRecord {
    LivestreamAlertRecord {
        alert_id: id.into(),
        alert_type,
        title: "Title".into(),
        message: "Message".into(),
        reason: "Reason".into(),
        confidence: 0.9,
        created_at,
        extra: Default::default(),
    }
}

fn alerted(entries: &[(&str, i64)]) -> Option<IndexMap<String, i64>> {
    Some(entries.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect())
}

#[test]
fn does_not_reapply_the_generic_semantic_floor_to_a_confirmed_destiny_guest() {
    let floor = livestream_alert_confidence_floor(LivestreamAlertType::DestinyGuest, "", 0.62);
    assert_eq!(floor, 0.62);
    assert!(0.705_942_983_138_825_1 >= floor);
}

#[test]
fn keeps_the_ordinary_semantic_alert_floor() {
    assert_eq!(
        livestream_alert_confidence_floor(LivestreamAlertType::Debate, "", 0.62),
        0.75
    );
}

#[test]
fn raises_ordinary_alert_confidence_after_repeated_negative_feedback() {
    assert_eq!(
        livestream_alert_confidence_floor(
            LivestreamAlertType::Debate,
            "debate:not_useful\ndebate:false_positive",
            0.62
        ),
        0.9
    );
}

#[test]
fn raises_only_the_dedicated_destiny_floor_after_negative_feedback() {
    assert_eq!(
        livestream_alert_confidence_floor(
            LivestreamAlertType::DestinyGuest,
            "destiny_guest:not_useful\ndestiny_guest:false_positive",
            0.62
        ),
        0.75
    );
}

#[test]
fn retains_per_type_dedup_when_another_alert_becomes_latest() {
    let state = LivestreamIntelligenceData {
        alerted_at_by_type: alerted(&[("destiny_guest", 1_500)]),
        latest_alert: Some(alert("later", LivestreamAlertType::Debate, 1_800)),
        ..state()
    };
    assert!(alert_sent_in_session(
        &state,
        LivestreamAlertType::DestinyGuest
    ));
}

#[test]
fn does_not_carry_dedup_into_a_newer_session() {
    let state = LivestreamIntelligenceData {
        session_started_at: 2_000,
        alerted_at_by_type: alerted(&[("destiny_guest", 1_500)]),
        ..state()
    };
    assert!(!alert_sent_in_session(
        &state,
        LivestreamAlertType::DestinyGuest
    ));
}

#[test]
fn durably_deduplicates_viewer_surges_by_type_within_the_session() {
    let state = LivestreamIntelligenceData {
        alerted_at_by_type: alerted(&[("viewer_surge", 1_500)]),
        latest_alert: Some(alert("later", LivestreamAlertType::DestinyGuest, 1_800)),
        ..state()
    };
    assert!(alert_sent_in_session(
        &state,
        LivestreamAlertType::ViewerSurge
    ));
}
