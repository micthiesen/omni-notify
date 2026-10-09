//! Port of `src/live-check/metrics/windows.spec.ts` (system time
//! 2024-06-15T12:00:00Z passed explicitly; local zone America/Vancouver).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use jiff::tz::TimeZone;
use omni_live::metrics::{
    DailyBucket, MetricWindow, calculate_window_max, prune_buckets, update_daily_bucket,
};

const NOW: i64 = 1_718_452_800_000;

fn tz() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

fn bucket(date: &str, max_viewers: i64, timestamp: i64) -> DailyBucket {
    DailyBucket {
        date: date.into(),
        max_viewers,
        timestamp,
    }
}

#[test]
fn should_create_a_new_bucket_for_today_when_none_exists() {
    let buckets = update_daily_bucket(&[], 1000, NOW);
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].date, "2024-06-15");
    assert_eq!(buckets[0].max_viewers, 1000);
}

#[test]
fn should_update_todays_bucket_when_new_count_is_higher() {
    let buckets = update_daily_bucket(&[bucket("2024-06-15", 500, 100)], 1000, NOW);
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].max_viewers, 1000);
}

#[test]
fn should_not_update_todays_bucket_when_new_count_is_lower() {
    let buckets = update_daily_bucket(&[bucket("2024-06-15", 1000, 100)], 500, NOW);
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].max_viewers, 1000);
    assert_eq!(buckets[0].timestamp, 100);
}

#[test]
fn should_preserve_existing_buckets_from_other_days() {
    let buckets = update_daily_bucket(
        &[
            bucket("2024-06-14", 800, 100),
            bucket("2024-06-13", 600, 50),
        ],
        1000,
        NOW,
    );
    assert_eq!(buckets.len(), 3);
    assert_eq!(
        buckets
            .iter()
            .find(|b| b.date == "2024-06-15")
            .unwrap()
            .max_viewers,
        1000
    );
    assert_eq!(
        buckets
            .iter()
            .find(|b| b.date == "2024-06-14")
            .unwrap()
            .max_viewers,
        800
    );
}

#[test]
fn should_not_mutate_the_original_array() {
    let existing = vec![bucket("2024-06-15", 500, 100)];
    let result = update_daily_bucket(&existing, 1000, NOW);
    assert_eq!(existing[0].max_viewers, 500);
    assert_eq!(result[0].max_viewers, 1000);
}

#[test]
fn should_remove_buckets_older_than_max_days() {
    let buckets = vec![
        bucket("2024-06-15", 1000, 100),
        bucket("2024-06-10", 800, 50),
        bucket("2024-06-01", 600, 30),
        bucket("2024-05-01", 400, 10),
    ];
    let pruned = prune_buckets(&buckets, 30, NOW, &tz());
    let dates: Vec<&str> = pruned.iter().map(|b| b.date.as_str()).collect();
    assert_eq!(dates, ["2024-06-15", "2024-06-10", "2024-06-01"]);
}

#[test]
fn should_keep_all_buckets_if_none_are_too_old() {
    let buckets = vec![
        bucket("2024-06-15", 1000, 100),
        bucket("2024-06-14", 800, 50),
    ];
    assert_eq!(prune_buckets(&buckets, 100, NOW, &tz()).len(), 2);
}

fn base() -> Vec<DailyBucket> {
    vec![
        bucket("2024-06-15", 1000, 100),
        bucket("2024-06-10", 1500, 80),
        bucket("2024-06-01", 2000, 60),
        bucket("2024-05-01", 3000, 40),
        bucket("2024-04-01", 5000, 20),
    ]
}

#[test]
fn should_return_all_time_max_for_all_time_window() {
    assert_eq!(
        calculate_window_max(&base(), 10000, MetricWindow::AllTime.config(), NOW, &tz()),
        10000
    );
}

#[test]
fn should_calculate_7_day_max_correctly() {
    assert_eq!(
        calculate_window_max(&base(), 10000, MetricWindow::SevenDays.config(), NOW, &tz()),
        1500
    );
}

#[test]
fn should_calculate_30_day_max_correctly() {
    assert_eq!(
        calculate_window_max(
            &base(),
            10000,
            MetricWindow::ThirtyDays.config(),
            NOW,
            &tz()
        ),
        2000
    );
}

#[test]
fn should_calculate_90_day_max_correctly() {
    assert_eq!(
        calculate_window_max(
            &base(),
            10000,
            MetricWindow::NinetyDays.config(),
            NOW,
            &tz()
        ),
        5000
    );
}

#[test]
fn should_return_0_for_empty_buckets_in_non_all_time_windows() {
    assert_eq!(
        calculate_window_max(&[], 10000, MetricWindow::SevenDays.config(), NOW, &tz()),
        0
    );
}
