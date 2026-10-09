//! Platform outage detection and alerts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_live::outage::{OutageAlert, OutageAlerter, OutageKind, UnknownStreak};

const MINUTE: i64 = 60_000;
const TIMEOUT: &str = "Timeout awaiting 'request' for 10000ms";

fn streak(name: &str, ticks: u32) -> UnknownStreak {
    streak_err(name, ticks, TIMEOUT)
}

fn streak_err(name: &str, ticks: u32, error: &str) -> UnknownStreak {
    UnknownStreak {
        display_name: name.into(),
        ticks,
        error: error.into(),
    }
}

fn clean(alerter: &mut OutageAlerter, count: i64, start: i64) -> Vec<Option<OutageAlert>> {
    (0..count)
        .map(|i| alerter.evaluate(&[], 4, start + i * 20_000))
        .collect()
}

#[test]
fn stays_quiet_below_the_tick_threshold() {
    let mut a = OutageAlerter::default();
    assert_eq!(a.evaluate(&[streak("Radiant", 1)], 4, 0), None);
    assert_eq!(a.evaluate(&[streak("Radiant", 2)], 4, 20_000), None);
}

#[test]
fn alerts_once_when_the_outage_is_confirmed() {
    let mut a = OutageAlerter::default();
    assert_eq!(
        a.evaluate(&[streak("Radiant", 3), streak("AnythingElse", 3)], 4, 0),
        Some(OutageAlert {
            kind: OutageKind::Degraded,
            title: "Live check degraded: 2/4 streamers unreachable".into(),
            message: format!("Radiant, AnythingElse\n{TIMEOUT}"),
        })
    );
}

#[test]
fn does_not_re_alert_on_every_subsequent_tick() {
    let mut a = OutageAlerter::default();
    a.evaluate(&[streak("Radiant", 3)], 4, 0);
    assert_eq!(a.evaluate(&[streak("Radiant", 4)], 4, 20_000), None);
    assert_eq!(a.evaluate(&[streak("Radiant", 20)], 4, 10 * MINUTE), None);
}

#[test]
fn escalates_on_a_widening_schedule_while_the_outage_persists() {
    let mut a = OutageAlerter::default();
    a.evaluate(&[streak("Radiant", 3)], 4, 0);
    let second = a
        .evaluate(&[streak("Radiant", 100)], 4, 30 * MINUTE)
        .unwrap();
    assert_eq!(second.kind, OutageKind::Degraded);
    assert!(second.message.contains("Unreachable for 30m."));
    assert_eq!(a.evaluate(&[streak("Radiant", 200)], 4, 60 * MINUTE), None);
    assert_eq!(
        a.evaluate(&[streak("Radiant", 400)], 4, 150 * MINUTE)
            .map(|x| x.kind),
        Some(OutageKind::Degraded)
    );
}

#[test]
fn sends_one_recovery_notice_once_the_fleet_stays_clean() {
    let mut a = OutageAlerter::default();
    a.evaluate(&[streak("Radiant", 3)], 4, 0);
    let alerts = clean(&mut a, 3, 12 * MINUTE);
    assert_eq!(alerts[0], None);
    assert_eq!(alerts[1], None);
    assert_eq!(
        alerts[2],
        Some(OutageAlert {
            kind: OutageKind::Recovered,
            title: "Live check recovered".into(),
            message: "All streamers reachable again after 13m.".into(),
        })
    );
    assert_eq!(a.evaluate(&[], 4, 20 * MINUTE), None);
}

#[test]
fn stays_silent_for_a_blip_that_never_reached_the_threshold() {
    let mut a = OutageAlerter::default();
    a.evaluate(&[streak("Radiant", 2)], 4, 0);
    assert_eq!(clean(&mut a, 3, MINUTE), vec![None, None, None]);
}

#[test]
fn does_not_emit_degraded_recovered_pairs_for_a_flapping_streamer() {
    let mut a = OutageAlerter::default();
    assert_eq!(
        a.evaluate(&[streak("Radiant", 3)], 4, 0).map(|x| x.kind),
        Some(OutageKind::Degraded)
    );
    for cycle in 1..=5 {
        let base = cycle * MINUTE;
        assert_eq!(a.evaluate(&[], 4, base), None);
        assert_eq!(a.evaluate(&[streak("Radiant", 3)], 4, base + 20_000), None);
        assert_eq!(a.evaluate(&[streak("Radiant", 4)], 4, base + 40_000), None);
    }
}

#[test]
fn restarts_escalation_for_a_genuinely_new_outage_after_recovery() {
    let mut a = OutageAlerter::default();
    a.evaluate(&[streak("Radiant", 3)], 4, 0);
    clean(&mut a, 3, MINUTE);
    assert_eq!(
        a.evaluate(&[streak("Radiant", 3)], 4, 5 * MINUTE)
            .map(|x| x.kind),
        Some(OutageKind::Degraded)
    );
}

#[test]
fn summarizes_large_outages_with_a_name_tail_and_distinct_errors() {
    let mut a = OutageAlerter::default();
    let streaks = [
        streak("A", 9),
        streak("B", 8),
        streak("C", 7),
        streak("D", 6),
        streak("E", 5),
        streak_err("F", 4, "ECONNREFUSED"),
        streak_err("G", 3, "403 Forbidden"),
    ];
    let alert = a.evaluate(&streaks, 7, 0).unwrap();
    assert_eq!(
        alert.title,
        "Live check degraded: 7/7 streamers unreachable"
    );
    assert_eq!(
        alert.message,
        [
            "A, B, C, D, E +2 more",
            TIMEOUT,
            "ECONNREFUSED",
            "+1 other error(s)"
        ]
        .join("\n")
    );
}
