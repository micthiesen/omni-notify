//! Missed-run recovery decisions (`src/task-runs/catchUp.ts`).

use jiff::Timestamp;

use crate::CronSchedule;

const HOUR_MS: i64 = 60 * 60 * 1000;
const MIN_CATCH_UP_CADENCE_MS: i64 = 6 * HOUR_MS;
const MIN_CATCH_UP_WINDOW_MS: i64 = 6 * HOUR_MS;
const MAX_CATCH_UP_WINDOW_MS: i64 = 48 * HOUR_MS;
const OCCURRENCES_PER_SIDE: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchUpDecision {
    None,
    Disabled {
        cadence_ms: i64,
    },
    Stale {
        scheduled_for: i64,
        lateness_ms: i64,
        max_lateness_ms: i64,
    },
    Run {
        scheduled_for: i64,
        lateness_ms: i64,
        max_lateness_ms: i64,
    },
}

/// Whether the newest occurrence since `evaluated_through` should run now. At
/// most one occurrence is recovered regardless of backlog. Without a persisted
/// cursor there is nothing to recover.
pub fn decide(s: &CronSchedule, now: Timestamp, evaluated_through: Option<i64>) -> CatchUpDecision {
    let Some(evaluated_through) = evaluated_through else {
        return CatchUpDecision::None;
    };
    let now_ms = now.as_millisecond();
    let occurrences = nearby_occurrences(s, now);
    let Some(previous) = occurrences.iter().copied().filter(|t| *t <= now_ms).max() else {
        return CatchUpDecision::None;
    };
    if previous <= evaluated_through {
        return CatchUpDecision::None;
    }
    let cadence_ms = occurrences
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .min()
        .unwrap_or(i64::MAX);
    if cadence_ms <= MIN_CATCH_UP_CADENCE_MS {
        return CatchUpDecision::Disabled { cadence_ms };
    }
    let max_lateness_ms = MAX_CATCH_UP_WINDOW_MS.min(MIN_CATCH_UP_WINDOW_MS.max(cadence_ms / 4));
    let lateness_ms = now_ms - previous;
    if lateness_ms > max_lateness_ms {
        CatchUpDecision::Stale {
            scheduled_for: previous,
            lateness_ms,
            max_lateness_ms,
        }
    } else {
        CatchUpDecision::Run {
            scheduled_for: previous,
            lateness_ms,
            max_lateness_ms,
        }
    }
}

/// Up to four occurrences strictly before `now` and four strictly after, sorted
/// (cron-parser `prev()` / `next()` from `currentDate = now`).
fn nearby_occurrences(s: &CronSchedule, now: Timestamp) -> Vec<i64> {
    let mut occurrences = Vec::with_capacity(OCCURRENCES_PER_SIDE * 2);
    let mut cursor = now;
    for _ in 0..OCCURRENCES_PER_SIDE {
        let Some(before) = cursor.checked_sub(jiff::SignedDuration::from_nanos(1)).ok() else {
            break;
        };
        let Some(previous) = s.prev_at_or_before(before) else {
            break;
        };
        occurrences.push(previous.as_millisecond());
        cursor = previous;
    }
    occurrences.extend(
        s.next_n(now, OCCURRENCES_PER_SIDE)
            .iter()
            .map(|t| t.as_millisecond()),
    );
    occurrences.sort_unstable();
    occurrences
}
