//! Port of `src/task-runs/catchUp.spec.ts` (local times in America/Vancouver).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use jiff::Timestamp;
use jiff::civil::date;
use jiff::tz::TimeZone;
use omni_tasks::CronSchedule;
use omni_tasks::catch_up::{CatchUpDecision, decide};

const HOUR_MS: i64 = 60 * 60 * 1000;

fn tz() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

/// `new Date(2026, month0, day, hour, minute)` in local time.
fn local(month0: i8, day: i8, hour: i8, minute: i8) -> i64 {
    date(2026, month0 + 1, day)
        .at(hour, minute, 0, 0)
        .to_zoned(tz())
        .unwrap()
        .timestamp()
        .as_millisecond()
}

fn local_time(day: i8, hour: i8) -> i64 {
    local(6, day, hour, 0)
}

fn run(schedule: &str, evaluated_through: i64, now: i64) -> CatchUpDecision {
    let schedule = CronSchedule::parse(schedule, &tz()).unwrap();
    decide(
        &schedule,
        Timestamp::from_millisecond(now).unwrap(),
        Some(evaluated_through),
    )
}

#[test]
fn does_not_recover_minutely_or_hourly_schedules() {
    for schedule in ["0 * * * * *", "0 0 * * * *"] {
        let now = local(6, 15, 10, 30);
        assert!(
            matches!(
                run(schedule, now - 2 * HOUR_MS, now),
                CatchUpDecision::Disabled { .. }
            ),
            "{schedule}"
        );
    }
}

#[test]
fn recovers_a_daily_task_up_to_six_hours_late() {
    let decision = run("0 0 5 * * *", local_time(14, 6), local_time(15, 10));
    assert_eq!(
        decision,
        CatchUpDecision::Run {
            scheduled_for: local_time(15, 5),
            lateness_ms: 5 * HOUR_MS,
            max_lateness_ms: 6 * HOUR_MS,
        }
    );
}

#[test]
fn skips_a_daily_task_more_than_six_hours_late() {
    let decision = run("0 0 5 * * *", local_time(14, 6), local_time(15, 12));
    assert!(matches!(decision, CatchUpDecision::Stale { .. }));
}

#[test]
fn uses_a_twelve_hour_window_for_a_mon_wed_fri_task() {
    let decision = run("0 0 5 * * 1,3,5", local_time(15, 6), local_time(17, 12));
    assert_eq!(
        decision,
        CatchUpDecision::Run {
            scheduled_for: local_time(17, 5),
            lateness_ms: 7 * HOUR_MS,
            max_lateness_ms: 12 * HOUR_MS,
        }
    );
}

#[test]
fn recovers_a_weekly_task_within_its_42_hour_window() {
    let decision = run("0 0 4 * * 0", local_time(5, 5), local_time(13, 16));
    assert_eq!(
        decision,
        CatchUpDecision::Run {
            scheduled_for: local_time(12, 4),
            lateness_ms: 36 * HOUR_MS,
            max_lateness_ms: 42 * HOUR_MS,
        }
    );
}

#[test]
fn caps_the_catch_up_window_at_48_hours() {
    let within = run("0 0 0 1 * *", local_time(1, 1), local(7, 2, 23, 0));
    let outside = run("0 0 0 1 * *", local_time(1, 1), local(7, 3, 1, 0));
    assert!(matches!(
        within,
        CatchUpDecision::Run {
            max_lateness_ms,
            ..
        } if max_lateness_ms == 48 * HOUR_MS
    ));
    assert!(matches!(
        outside,
        CatchUpDecision::Stale {
            max_lateness_ms,
            ..
        } if max_lateness_ms == 48 * HOUR_MS
    ));
}

#[test]
fn does_nothing_when_the_newest_occurrence_was_already_evaluated() {
    assert_eq!(
        run("0 0 5 * * *", local_time(15, 5), local_time(15, 10)),
        CatchUpDecision::None
    );
}

#[test]
fn nothing_to_recover_without_a_cursor() {
    let schedule = CronSchedule::parse("0 0 5 * * *", &tz()).unwrap();
    let now = Timestamp::from_millisecond(local_time(15, 10)).unwrap();
    assert_eq!(decide(&schedule, now, None), CatchUpDecision::None);
}
