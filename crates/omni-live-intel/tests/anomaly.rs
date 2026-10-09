//! Viewer-surge anomaly detection.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashSet;

use omni_live_intel::anomaly::{
    AnomalyInput, ViewerAnomalyTracker, compute_relevance, typical_session_peak,
};
use omni_live_intel::observation::{DggPresence, Streamer, StreamerTier};
use omni_live_intel::types::{SemanticMetadata, StreamSession, ViewerTrend};

const MINUTE: i64 = 60_000;

fn streamer() -> Streamer {
    Streamer {
        id: "hutch".into(),
        display_name: "Hutch".into(),
        bindings: vec![],
        tier: StreamerTier::Background,
        dgg: Some(DggPresence {
            hosted: true,
            viewers: Some(45.0),
        }),
    }
}

fn input(id: &str, viewers: Option<f64>, dgg: Option<f64>, now: i64) -> AnomalyInput<'_> {
    AnomalyInput {
        streamer_id: id,
        viewers,
        dgg_viewers: dgg,
        session_started_at: 0,
        now,
        ..AnomalyInput::default()
    }
}

fn observe_stable_baseline(tracker: &mut ViewerAnomalyTracker, minutes: i64) {
    for minute in 0..minutes {
        tracker.observe(input("hutch", Some(200.0), Some(30.0), minute * MINUTE));
    }
}

fn suppression(trend: &ViewerTrend) -> Option<String> {
    trend.suppression_reason.clone().flatten()
}

#[test]
fn suppresses_the_normal_audience_ramp_during_the_first_twenty_minutes() {
    let mut tracker = ViewerAnomalyTracker::new();
    for minute in 0..20 {
        let (viewers, dgg) = if minute == 0 {
            (1.0, 2.0)
        } else {
            (400.0, 100.0)
        };
        let trend = tracker.observe(input("hutch", Some(viewers), Some(dgg), minute * MINUTE));
        assert!(!trend.anomalous);
        assert!(
            suppression(&trend)
                .unwrap_or_default()
                .contains("Building a post-start baseline")
        );
    }
}

#[test]
fn never_flags_a_gradual_post_start_ramp_even_after_the_warmup() {
    let mut tracker = ViewerAnomalyTracker::new();
    let mut reasons = HashSet::new();
    for minute in 0..=90_i64 {
        #[allow(clippy::cast_precision_loss)]
        let viewers = (1_500.0 * (1.0 - (-(minute as f64) / 15.0).exp())).round();
        let trend = tracker.observe(AnomalyInput {
            total_viewers: Some(viewers),
            ..input(
                "anythingelse",
                Some(viewers),
                Some((viewers / 8.0).round()),
                minute * MINUTE,
            )
        });
        assert!(!trend.anomalous);
        reasons.insert(suppression(&trend));
    }
    assert!(reasons.contains(&Some("Audience is still ramping up".to_owned())));
}

#[test]
fn flags_a_sudden_surge_against_a_flat_baseline() {
    let mut tracker = ViewerAnomalyTracker::new();
    observe_stable_baseline(&mut tracker, 25);
    let candidate = tracker.observe(input("hutch", Some(430.0), Some(70.0), 25 * MINUTE));
    assert!(!candidate.anomalous);
    assert!(
        suppression(&candidate)
            .unwrap_or_default()
            .contains("another observation")
    );
    let trend = tracker.observe(input("hutch", Some(440.0), Some(72.0), 26 * MINUTE));
    assert!(trend.anomalous);
    let reason = trend.reason.unwrap_or_default();
    assert!(reason.contains("viewers up"));
    assert!(reason.contains("200 baseline"));
    assert!(reason.contains("DGG audience up"));
}

#[test]
fn requires_a_platform_surge_to_reach_the_typical_session_peak() {
    let observe_surge = |typical_peak: f64| {
        let mut tracker = ViewerAnomalyTracker::new();
        let mut trend = None;
        for minute in 0..27 {
            let viewers = if minute < 25 { 500.0 } else { 900.0 };
            trend = Some(tracker.observe(AnomalyInput {
                total_viewers: Some(viewers + 100.0),
                typical_peak: Some(typical_peak),
                ..input("anythingelse", Some(viewers), None, minute * MINUTE)
            }));
        }
        trend.expect("observed")
    };
    let ordinary = observe_surge(2_500.0);
    assert!(!ordinary.anomalous);
    assert_eq!(
        suppression(&ordinary).as_deref(),
        Some("Below the typical session peak of 2500")
    );
    assert_eq!(ordinary.typical_peak_viewers, Some(Some(2_500.0)));
    assert!(observe_surge(950.0).anomalous);
}

#[test]
fn restarts_the_baseline_when_the_primary_binding_changes() {
    let mut tracker = ViewerAnomalyTracker::new();
    for minute in 0..27 {
        let switched = minute >= 25;
        let trend = tracker.observe(AnomalyInput {
            source_key: Some(
                if switched {
                    "kick:destiny"
                } else {
                    "youtube:destiny"
                }
                .into(),
            ),
            ..input(
                "destiny",
                Some(if switched { 3_000.0 } else { 1_000.0 }),
                None,
                minute * MINUTE,
            )
        });
        assert!(!trend.anomalous);
    }
}

#[test]
fn does_not_trust_a_short_post_restart_history_to_rule_out_a_ramp() {
    let mut tracker = ViewerAnomalyTracker::new();
    let tick = MINUTE / 3;
    let mut at = 22 * MINUTE;
    while at < 32 * MINUTE {
        let viewers = if at < 24 * MINUTE + MINUTE / 2 {
            1_000.0
        } else {
            1_650.0
        };
        let trend = tracker.observe(input("anythingelse", Some(viewers), None, at));
        assert!(!trend.anomalous);
        at += tick;
    }
}

#[test]
fn keeps_the_dgg_baseline_across_a_primary_switch() {
    let mut tracker = ViewerAnomalyTracker::new();
    let mut trend = None;
    for minute in 0..27 {
        trend = Some(
            tracker.observe(AnomalyInput {
                source_key: Some(
                    if minute < 24 {
                        "youtube:hutch"
                    } else {
                        "kick:hutch"
                    }
                    .into(),
                ),
                ..input(
                    "hutch",
                    Some(200.0),
                    Some(if minute < 25 { 30.0 } else { 90.0 }),
                    minute * MINUTE,
                )
            }),
        );
    }
    let trend = trend.expect("observed");
    assert!(trend.anomalous);
    assert!(trend.reason.unwrap_or_default().contains("DGG audience up"));
}

#[test]
fn does_not_confirm_a_one_observation_scrape_spike() {
    let mut tracker = ViewerAnomalyTracker::new();
    observe_stable_baseline(&mut tracker, 25);
    let mut sample = |viewers: f64, minute: i64| {
        tracker.observe(input("hutch", Some(viewers), Some(30.0), minute * MINUTE))
    };
    assert!(!sample(430.0, 25).anomalous);
    assert!(!sample(205.0, 26).anomalous);
    assert!(!sample(430.0, 27).anomalous);
}

#[test]
fn does_not_combine_unrelated_platform_and_dgg_spikes_into_confirmation() {
    let mut tracker = ViewerAnomalyTracker::new();
    observe_stable_baseline(&mut tracker, 25);
    assert!(
        !tracker
            .observe(input("hutch", Some(430.0), Some(30.0), 25 * MINUTE))
            .anomalous
    );
    assert!(
        !tracker
            .observe(input("hutch", Some(200.0), Some(70.0), 26 * MINUTE))
            .anomalous
    );
}

#[test]
fn retains_a_prsek_shaped_late_seventy_one_percent_surge() {
    let mut tracker = ViewerAnomalyTracker::new();
    for minute in 0..25 {
        tracker.observe(input("prsek", Some(150.0), Some(30.0), minute * MINUTE));
    }
    tracker.observe(input("prsek", Some(250.0), Some(30.0), 25 * MINUTE));
    let trend = tracker.observe(input("prsek", Some(256.0), Some(30.0), 26 * MINUTE));
    assert!(trend.anomalous);
    assert!((trend.percent_change - 70.67).abs() < 0.005);
}

#[test]
fn does_not_turn_missing_platform_viewer_data_into_a_synthetic_zero() {
    let mut tracker = ViewerAnomalyTracker::new();
    for minute in 0..25 {
        let viewers = if minute == 10 { None } else { Some(200.0) };
        tracker.observe(input("hutch", viewers, None, minute * MINUTE));
    }
    let trend = tracker.observe(input("hutch", Some(220.0), None, 25 * MINUTE));
    assert!(!trend.anomalous);
    assert_eq!(trend.percent_change, 10.0);
}

#[test]
fn clears_confirmation_evidence_between_sessions() {
    let mut tracker = ViewerAnomalyTracker::new();
    observe_stable_baseline(&mut tracker, 25);
    tracker.observe(input("hutch", Some(430.0), Some(30.0), 25 * MINUTE));
    tracker.clear("hutch");
    let next_session = tracker.observe(AnomalyInput {
        session_started_at: 26 * MINUTE,
        ..input("hutch", Some(430.0), Some(30.0), 26 * MINUTE)
    });
    assert!(!next_session.anomalous);
    assert_eq!(next_session.candidate_observations, Some(0.0));
}

const DAY: i64 = 24 * 60 * MINUTE;

#[allow(clippy::cast_precision_loss)]
fn session(ended_at: i64, peak_viewers: f64, minutes: i64) -> StreamSession {
    StreamSession {
        started_at: (ended_at - minutes * MINUTE) as f64,
        ended_at: ended_at as f64,
        duration_ms: (minutes * MINUTE) as f64,
        peak_viewers,
        title: "stream".into(),
        platform: "kick".into(),
        username: "anythingelse".into(),
        extra: Default::default(),
    }
}

#[test]
fn uses_the_median_peak_of_recent_full_sessions() {
    let now = 100 * DAY;
    assert_eq!(
        typical_session_peak(
            &[
                session(now - 40 * DAY, 9_000.0, 120),
                session(now - 3 * DAY, 2_400.0, 120),
                session(now - 2 * DAY, 150.0, 5),
                session(now - 2 * DAY, 2_600.0, 120),
                session(now - DAY, 3_600.0, 120),
            ],
            now
        ),
        Some(2_600.0)
    );
}

#[test]
fn returns_null_without_three_qualifying_sessions() {
    let now = 100 * DAY;
    assert_eq!(
        typical_session_peak(
            &[
                session(now - DAY, 2_000.0, 120),
                session(now - DAY, 300.0, 10)
            ],
            now
        ),
        None
    );
}

#[test]
fn makes_confirmed_destiny_presence_decisive() {
    let (score, reasons) = compute_relevance(&streamer(), None, None, true);
    assert!(score >= 60.0);
    assert!(reasons.contains(&"Destiny detected as a live participant".to_owned()));
}

#[test]
fn combines_semantic_importance_audience_and_anomaly() {
    let semantic = SemanticMetadata {
        headline: "A live debate is beginning".into(),
        topics: vec!["debate".into()],
        content_kind: "debate".into(),
        importance: 90.0,
        reason: "substantive debate".into(),
        updated_at: 1,
        extra: Default::default(),
    };
    let trend = ViewerTrend {
        percent_change: 70.0,
        viewers_per_minute: 20.0,
        dgg_percent_change: Some(120.0),
        anomalous: true,
        reason: Some("viewers up 70%".into()),
        updated_at: 1,
        ..ViewerTrend::default()
    };
    let (score, reasons) = compute_relevance(&streamer(), Some(&semantic), Some(&trend), false);
    assert!(score >= 80.0);
    assert!(reasons.contains(&"substantive debate".to_owned()));
}
