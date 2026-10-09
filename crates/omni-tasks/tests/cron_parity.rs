//! Scheduler fire times for every schedule in the task map against committed
//! reference times (`tests/golden/cron-parity.json`), evaluated in
//! America/Vancouver. Slow schedules cover all of 2026; sub-hourly ones cover
//! the 2026 DST transitions.
//!
//! The reference fires the first time-of-day match after a spring-forward
//! transition one hour late (04:00 PDT is recorded as 05:00 PDT); those entries
//! are listed in `REFERENCE_SPRING_FORWARD_BUG` and the scheduler fires at the
//! correct wall time. Every other occurrence, including wall times inside the
//! gap (shifted forward) and the repeated fall-back hour, is identical.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use omni_tasks::CronSchedule;
use serde::Deserialize;

/// `(expression, scheduler fire, reference fire)`, one hour apart.
const REFERENCE_SPRING_FORWARD_BUG: &[(&str, &str, &str)] = &[
    (
        "0 */6 * * *",
        "2026-03-08T13:00:00.000Z",
        "2026-03-08T14:00:00.000Z",
    ),
    (
        "0 0 */6 * * *",
        "2026-03-08T13:00:00.000Z",
        "2026-03-08T14:00:00.000Z",
    ),
    (
        "0 0 11 * * 1,3,5",
        "2026-03-09T18:00:00.000Z",
        "2026-03-09T19:00:00.000Z",
    ),
    (
        "0 0 17 * * 1,3,5",
        "2026-03-10T00:00:00.000Z",
        "2026-03-10T01:00:00.000Z",
    ),
    (
        "0 0 4 * * 0",
        "2026-03-08T11:00:00.000Z",
        "2026-03-08T12:00:00.000Z",
    ),
    (
        "0 0 5 * * 0",
        "2026-03-08T12:00:00.000Z",
        "2026-03-08T13:00:00.000Z",
    ),
    (
        "0 0 9 * * 0",
        "2026-03-08T16:00:00.000Z",
        "2026-03-08T17:00:00.000Z",
    ),
    (
        "0 30 1 * * *",
        "2026-03-09T08:30:00.000Z",
        "2026-03-09T09:30:00.000Z",
    ),
];

#[derive(Deserialize)]
struct Window {
    start: i64,
    end: i64,
    next: Vec<i64>,
}

fn rust_sequence(schedule: &CronSchedule, start: i64, end: i64) -> Vec<i64> {
    let mut out = Vec::new();
    let mut cursor = Timestamp::from_millisecond(start).unwrap();
    while let Some(next) = schedule.next_after(cursor) {
        if next.as_millisecond() > end {
            break;
        }
        out.push(next.as_millisecond());
        cursor = next;
    }
    out
}

#[test]
fn scheduler_matches_the_reference_fires_for_the_task_map() {
    let raw = include_str!("golden/cron-parity.json");
    let fixture: BTreeMap<String, Vec<Window>> = serde_json::from_str(raw).unwrap();
    let tz = TimeZone::get("America/Vancouver").unwrap();
    let mut divergences = Vec::new();
    for (expr, windows) in &fixture {
        let schedule = CronSchedule::parse(expr, &tz).unwrap();
        for window in windows {
            let ours = rust_sequence(&schedule, window.start, window.end);
            let only_ours: Vec<String> = ours
                .iter()
                .filter(|t| !window.next.contains(t))
                .map(|t| omni_core::js::to_iso_string(*t))
                .collect();
            let only_reference: Vec<String> = window
                .next
                .iter()
                .filter(|t| !ours.contains(t))
                .map(|t| omni_core::js::to_iso_string(*t))
                .collect();
            assert_eq!(
                only_ours.len(),
                only_reference.len(),
                "{expr}: fire counts differ"
            );
            for (croner, reference) in only_ours.into_iter().zip(only_reference) {
                divergences.push((expr.clone(), croner, reference));
            }
        }
    }
    let expected: Vec<(String, String, String)> = REFERENCE_SPRING_FORWARD_BUG
        .iter()
        .map(|(e, c, f)| ((*e).to_owned(), (*c).to_owned(), (*f).to_owned()))
        .collect();
    assert_eq!(divergences, expected);
    for (_, croner, reference) in &divergences {
        let croner: Timestamp = croner.parse().unwrap();
        let reference: Timestamp = reference.parse().unwrap();
        assert_eq!(reference.as_second() - croner.as_second(), 3600);
    }
}
