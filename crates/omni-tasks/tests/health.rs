//! The persistent-failure rule and the incident transitions built on it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_store::cbor::Extra;
use omni_tasks::health::{
    IncidentDecision, PERSISTENT_FAILURE, TaskHealthIncident, decide_incident,
};
use omni_tasks::{TaskRunData, TaskRunStatus, Trigger};

const HOUR: i64 = 60 * 60_000;
const NOW: i64 = 100 * HOUR;

fn run(hours: i64, status: TaskRunStatus) -> TaskRunData {
    TaskRunData {
        run_id: format!("T:{hours}"),
        task_name: "T".into(),
        trigger: Trigger::Schedule,
        scheduled_for: None,
        started_at: hours * HOUR,
        finished_at: Some(hours * HOUR + 1000),
        status,
        error: match status {
            TaskRunStatus::Degraded => Some(format!("skipped at {hours}")),
            TaskRunStatus::Error => Some("boom".into()),
            _ => None,
        },
        summary: None,
        extra: Extra::default(),
    }
}

fn degraded(hours: i64) -> TaskRunData {
    run(hours, TaskRunStatus::Degraded)
}

fn err(hours: i64) -> TaskRunData {
    run(hours, TaskRunStatus::Error)
}

fn ok(hours: i64) -> TaskRunData {
    run(hours, TaskRunStatus::Success)
}

fn decide(runs: &[TaskRunData], incident: Option<&TaskHealthIncident>) -> IncidentDecision {
    decide_incident(&PERSISTENT_FAILURE, runs, incident, NOW)
}

#[test]
fn three_degraded_runs_within_twelve_hours_do_not_notify() {
    let runs = [degraded(20), degraded(14), degraded(9), ok(8)];
    assert_eq!(decide(&runs, None), IncidentDecision::Nothing);
}

#[test]
fn two_degraded_runs_spanning_a_day_do_not_notify() {
    assert_eq!(
        decide(&[degraded(30), degraded(6)], None),
        IncidentDecision::Nothing
    );
}

#[test]
fn three_bad_runs_spanning_twelve_hours_less_jitter_notify_with_the_latest_reason() {
    let mut newest = degraded(20);
    newest.started_at -= 5 * 60_000;
    let runs = [newest, err(14), degraded(8), ok(2)];
    let IncidentDecision::Notify(incident) = decide(&runs, None) else {
        panic!("expected a notification");
    };
    assert_eq!(incident.task_name, "T");
    assert_eq!(incident.first_bad_at, 8 * HOUR);
    assert_eq!(incident.notified_at, NOW);
    assert_eq!(incident.bad_runs, 3);
    assert_eq!(incident.reason.as_deref(), Some("skipped at 20"));
}

#[test]
fn a_streak_of_plain_errors_is_left_to_the_error_alert_path() {
    assert_eq!(
        decide(&[err(24), err(12), err(0)], None),
        IncidentDecision::Nothing
    );
}

#[test]
fn a_running_row_is_ignored_when_judging_a_settled_run() {
    let runs = [
        run(30, TaskRunStatus::Running),
        degraded(24),
        degraded(12),
        degraded(0),
    ];
    assert!(matches!(decide(&runs, None), IncidentDecision::Notify(_)));
}

#[test]
fn an_open_incident_notifies_once_while_the_streak_continues() {
    let runs = [degraded(36), degraded(24), degraded(12), degraded(0)];
    let IncidentDecision::Notify(incident) = decide(&runs[1..], None) else {
        panic!("expected a notification");
    };
    assert_eq!(decide(&runs, Some(&incident)), IncidentDecision::Nothing);
    // Pruned history (the oldest bad runs gone) is still the same incident.
    assert_eq!(
        decide(&runs[..2], Some(&incident)),
        IncidentDecision::Nothing
    );
}

#[test]
fn a_success_resolves_the_incident_once() {
    let runs = [degraded(24), degraded(12), degraded(0)];
    let IncidentDecision::Notify(incident) = decide(&runs, None) else {
        panic!("expected a notification");
    };
    let recovered = [ok(30), degraded(24), degraded(12), degraded(0)];
    assert_eq!(
        decide(&recovered, Some(&incident)),
        IncidentDecision::Recover(incident.clone())
    );
    // A missed success event is caught by a later bad run.
    let later = [degraded(31), ok(30), degraded(24)];
    assert_eq!(
        decide(&later, Some(&incident)),
        IncidentDecision::Recover(incident)
    );
    // After recovery the incident row is gone and a single bad run is quiet.
    assert_eq!(decide(&later, None), IncidentDecision::Nothing);
}

#[test]
fn the_rule_counts_failed_and_degraded_runs_and_stops_at_running_or_success() {
    assert!(PERSISTENT_FAILURE.is_persistent(&[degraded(24), err(12), degraded(0)]));
    assert!(!PERSISTENT_FAILURE.is_persistent(&[degraded(24), ok(12), degraded(0)]));
    assert!(!PERSISTENT_FAILURE.is_persistent(&[
        run(30, TaskRunStatus::Running),
        degraded(24),
        err(12),
        degraded(0),
    ]));
}

#[test]
fn old_rows_without_the_degraded_status_decode_as_before() {
    let row = serde_json::json!({
        "runId": "T:1", "taskName": "T", "trigger": "schedule",
        "startedAt": 1, "status": "success",
    });
    let run: TaskRunData = serde_json::from_value(row).unwrap();
    assert_eq!(run.status, TaskRunStatus::Success);
    let degraded: TaskRunStatus = serde_json::from_value(serde_json::json!("degraded")).unwrap();
    assert_eq!(degraded, TaskRunStatus::Degraded);
}
