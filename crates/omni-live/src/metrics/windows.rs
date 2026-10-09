//! Daily buckets and rolling windows (`metrics/windows.ts`, `metrics/types.ts`).

use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use serde::{Deserialize, Serialize};

/// One UTC day's peak.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyBucket {
    /// `YYYY-MM-DD` (UTC).
    pub date: String,
    pub max_viewers: i64,
    pub timestamp: i64,
}

/// The record windows, in notification-priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MetricWindow {
    SevenDays,
    ThirtyDays,
    NinetyDays,
    AllTime,
}

/// `WINDOW_CONFIGS[window]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowConfig {
    /// `None` is the all-time window.
    pub days: Option<i64>,
    pub label: &'static str,
    pub priority: u8,
}

impl MetricWindow {
    /// `Object.entries(WINDOW_CONFIGS)` order.
    pub const ALL: [MetricWindow; 4] = [
        MetricWindow::SevenDays,
        MetricWindow::ThirtyDays,
        MetricWindow::NinetyDays,
        MetricWindow::AllTime,
    ];

    pub fn config(self) -> WindowConfig {
        match self {
            MetricWindow::SevenDays => WindowConfig {
                days: Some(7),
                label: "7-day high",
                priority: 1,
            },
            MetricWindow::ThirtyDays => WindowConfig {
                days: Some(30),
                label: "30-day high",
                priority: 2,
            },
            MetricWindow::NinetyDays => WindowConfig {
                days: Some(90),
                label: "90-day high",
                priority: 3,
            },
            MetricWindow::AllTime => WindowConfig {
                days: None,
                label: "all-time record",
                priority: 4,
            },
        }
    }
}

/// `new Date(ms).toISOString().slice(0, 10)`.
pub fn to_date_stamp(ms: i64) -> String {
    omni_core::js::to_iso_string(ms).chars().take(10).collect()
}

/// `d = new Date(at); d.setDate(d.getDate() - days); toDateStamp(d)`:
/// calendar days back in the local zone, then the UTC date.
fn cutoff_stamp(at_ms: i64, days: i64, tz: &TimeZone) -> String {
    let Ok(at) = Timestamp::from_millisecond(at_ms) else {
        return to_date_stamp(at_ms);
    };
    let shifted = at
        .to_zoned(tz.clone())
        .checked_sub(days.days())
        .map_or(at_ms, |z| z.timestamp().as_millisecond());
    to_date_stamp(shifted)
}

/// Raises today's bucket or appends one; never mutates the input.
pub fn update_daily_bucket(
    buckets: &[DailyBucket],
    viewer_count: i64,
    observed_at: i64,
) -> Vec<DailyBucket> {
    let today = to_date_stamp(observed_at);
    let mut next = buckets.to_vec();
    match next.iter_mut().find(|b| b.date == today) {
        Some(existing) => {
            if viewer_count > existing.max_viewers {
                *existing = DailyBucket {
                    date: today,
                    max_viewers: viewer_count,
                    timestamp: observed_at,
                };
            }
        }
        None => next.push(DailyBucket {
            date: today,
            max_viewers: viewer_count,
            timestamp: observed_at,
        }),
    }
    next
}

/// Drops buckets older than `max_days`.
pub fn prune_buckets(
    buckets: &[DailyBucket],
    max_days: i64,
    observed_at: i64,
    tz: &TimeZone,
) -> Vec<DailyBucket> {
    let cutoff = cutoff_stamp(observed_at, max_days, tz);
    buckets
        .iter()
        .filter(|b| b.date.as_str() >= cutoff.as_str())
        .cloned()
        .collect()
}

/// The window maximum; the all-time window reads the stored all-time max.
pub fn calculate_window_max(
    buckets: &[DailyBucket],
    all_time_max: i64,
    window: WindowConfig,
    observed_at: i64,
    tz: &TimeZone,
) -> i64 {
    let Some(days) = window.days else {
        return all_time_max;
    };
    let cutoff = cutoff_stamp(observed_at, days, tz);
    buckets
        .iter()
        .filter(|b| b.date.as_str() >= cutoff.as_str())
        .map(|b| b.max_viewers)
        .fold(0, i64::max)
}
