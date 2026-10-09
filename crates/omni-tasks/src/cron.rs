//! Cron expressions evaluated in the configured time zone (croner + jiff).
//!
//! Five-field (`m h dom mon dow`) and six-field (`s m h dom mon dow`)
//! expressions are accepted. Matches have one-second resolution.
//!
//! DST: a wall time skipped by a spring-forward gap fires shifted forward by
//! the gap (02:30 becomes 03:30); repeated fall-back wall times follow the
//! absolute time line (see `tests/cron_parity.rs`).

use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use jiff::Timestamp;
use jiff::tz::TimeZone;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Backward,
}

/// A parsed cron expression bound to a time zone.
#[derive(Clone, Debug)]
pub struct CronSchedule {
    expr: String,
    cron: Cron,
    tz: TimeZone,
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidScheduleError {
    #[error("invalid cron expression {expr:?}: {reason}")]
    Invalid { expr: String, reason: String },
    #[error("cron expression {expr:?} never matches")]
    NeverMatches { expr: String },
}

impl CronSchedule {
    /// Parses 5- or 6-field cron; rejects expressions that never match
    /// (for example `0 0 0 30 2 *`).
    pub fn parse(expr: &str, tz: &TimeZone) -> Result<Self, InvalidScheduleError> {
        let invalid = |reason: String| InvalidScheduleError::Invalid {
            expr: expr.to_owned(),
            reason,
        };
        let fields = expr.split_whitespace().count();
        if !(5..=6).contains(&fields) {
            return Err(invalid(format!("expected 5 or 6 fields, found {fields}")));
        }
        let cron = CronParser::builder()
            .seconds(Seconds::Optional)
            .year(Year::Disallowed)
            .build()
            .parse(expr)
            .map_err(|e| invalid(e.to_string()))?;
        let schedule = Self {
            expr: expr.to_owned(),
            cron,
            tz: tz.clone(),
        };
        // An expression can parse yet never match (e.g. `0 0 0 30 2 *`);
        // croner's bounded search then fails, so probe once now instead of
        // inside the scheduler loop.
        let probe = Timestamp::UNIX_EPOCH.to_zoned(tz.clone());
        if schedule.cron.find_next_occurrence(&probe, false).is_err() {
            return Err(InvalidScheduleError::NeverMatches {
                expr: expr.to_owned(),
            });
        }
        Ok(schedule)
    }

    /// First match strictly after `t`.
    pub fn next_after(&self, t: Timestamp) -> Option<Timestamp> {
        let zoned = t.to_zoned(self.tz.clone());
        let found = self.cron.find_next_occurrence(&zoned, false).ok()?;
        if self.cron.is_time_matching(&found).unwrap_or(false) {
            return Some(found.timestamp());
        }
        // croner resolved a wall time inside a DST gap to the instant the gap
        // ends (and does so even for times before the gap). Resolve the
        // skipped wall time instead: shifted forward by
        // the gap length (jiff's `compatible` disambiguation).
        self.gap_walk(t, Direction::Forward)
    }

    /// Latest match at or before `t`.
    pub fn prev_at_or_before(&self, t: Timestamp) -> Option<Timestamp> {
        let zoned = t.to_zoned(self.tz.clone());
        let found = self.cron.find_previous_occurrence(&zoned, true).ok()?;
        if self.cron.is_time_matching(&found).unwrap_or(false) {
            return Some(found.timestamp()).filter(|found| *found <= t);
        }
        self.gap_walk(t, Direction::Backward)
    }

    /// Walks civil (wall clock) matches from `t` and resolves each to an
    /// instant with `compatible` disambiguation until one lies on the
    /// requested side of `t`.
    fn gap_walk(&self, t: Timestamp, direction: Direction) -> Option<Timestamp> {
        let mut civil = t.to_zoned(self.tz.clone()).datetime();
        let mut inclusive = direction == Direction::Backward;
        for _ in 0..8 {
            let matched = match direction {
                Direction::Forward => self.cron.find_next_occurrence(&civil, inclusive),
                Direction::Backward => self.cron.find_previous_occurrence(&civil, inclusive),
            }
            .ok()?;
            let instant = self
                .tz
                .to_ambiguous_zoned(matched)
                .compatible()
                .ok()?
                .timestamp();
            let accepted = match direction {
                Direction::Forward => instant > t,
                Direction::Backward => instant <= t,
            };
            if accepted {
                return Some(instant);
            }
            civil = matched;
            inclusive = false;
        }
        None
    }

    /// The next `n` matches after `t` (UI `nextRuns`).
    pub fn next_n(&self, t: Timestamp, n: usize) -> Vec<Timestamp> {
        let mut out = Vec::with_capacity(n);
        let mut cursor = t;
        while out.len() < n {
            let Some(next) = self.next_after(cursor) else {
                break;
            };
            out.push(next);
            cursor = next;
        }
        out
    }

    /// The expression as configured.
    pub fn as_str(&self) -> &str {
        &self.expr
    }

    /// The zone matches are evaluated in.
    pub fn time_zone(&self) -> &TimeZone {
        &self.tz
    }
}

impl PartialEq for CronSchedule {
    fn eq(&self, other: &Self) -> bool {
        self.expr == other.expr && self.tz == other.tz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vancouver() -> TimeZone {
        TimeZone::get("America/Vancouver").unwrap()
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn five_and_six_fields() {
        let tz = vancouver();
        let six = CronSchedule::parse("*/20 * * * * *", &tz).unwrap();
        assert_eq!(
            six.next_after(ts("2026-07-15T17:00:00.500Z")),
            Some(ts("2026-07-15T17:00:20Z"))
        );
        let five = CronSchedule::parse("*/5 * * * *", &tz).unwrap();
        assert_eq!(
            five.next_after(ts("2026-07-15T17:00:00Z")),
            Some(ts("2026-07-15T17:05:00Z"))
        );
        assert_eq!(five.as_str(), "*/5 * * * *");
    }

    #[test]
    fn local_time_zone() {
        let daily = CronSchedule::parse("0 0 5 * * *", &vancouver()).unwrap();
        // 05:00 PDT is 12:00 UTC.
        assert_eq!(
            daily.next_after(ts("2026-07-15T00:00:00Z")),
            Some(ts("2026-07-15T12:00:00Z"))
        );
        assert_eq!(
            daily.prev_at_or_before(ts("2026-07-15T12:00:00Z")),
            Some(ts("2026-07-15T12:00:00Z"))
        );
        assert_eq!(
            daily.prev_at_or_before(ts("2026-07-15T11:59:59.999Z")),
            Some(ts("2026-07-14T12:00:00Z"))
        );
    }

    #[test]
    fn rejects_invalid_and_never_matching() {
        let tz = vancouver();
        assert!(matches!(
            CronSchedule::parse("not a cron", &tz),
            Err(InvalidScheduleError::Invalid { .. })
        ));
        assert!(matches!(
            CronSchedule::parse("0 0 0 1 1 * 2030", &tz),
            Err(InvalidScheduleError::Invalid { .. })
        ));
        assert!(matches!(
            CronSchedule::parse("0 0 0 30 2 *", &tz),
            Err(InvalidScheduleError::NeverMatches { .. })
        ));
        assert!(CronSchedule::parse("0 0 0 29 2 *", &tz).is_ok());
    }

    #[test]
    fn spring_forward_gap_shifts_forward_without_spurious_fires() {
        let tz = vancouver();
        let gap = CronSchedule::parse("0 30 2 * * *", &tz).unwrap();
        // 02:30 does not exist on 2026-03-08; it fires at 03:30 PDT.
        assert_eq!(
            gap.next_after(ts("2026-03-08T08:00:00Z")),
            Some(ts("2026-03-08T10:30:00Z"))
        );
        assert_eq!(
            gap.prev_at_or_before(ts("2026-03-08T11:00:00Z")),
            Some(ts("2026-03-08T10:30:00Z"))
        );
        let before = CronSchedule::parse("0 30 1 * * *", &tz).unwrap();
        assert_eq!(
            before.next_after(ts("2026-03-08T09:30:00Z")),
            Some(ts("2026-03-09T08:30:00Z"))
        );
    }

    #[test]
    fn fall_back_hour_fires_on_both_passes() {
        let every_20 = CronSchedule::parse("0 */20 * * * *", &vancouver()).unwrap();
        // 01:00-02:00 repeats on 2026-11-01 (08:00Z-09:00Z PDT, 09:00Z-10:00Z PST).
        let runs = every_20.next_n(ts("2026-11-01T07:50:00Z"), 7);
        assert_eq!(
            runs,
            vec![
                ts("2026-11-01T08:00:00Z"),
                ts("2026-11-01T08:20:00Z"),
                ts("2026-11-01T08:40:00Z"),
                ts("2026-11-01T09:00:00Z"),
                ts("2026-11-01T09:20:00Z"),
                ts("2026-11-01T09:40:00Z"),
                ts("2026-11-01T10:00:00Z"),
            ]
        );
    }

    #[test]
    fn next_n_is_strictly_increasing() {
        let schedule = CronSchedule::parse("0 0 17 * * 1,3,5", &vancouver()).unwrap();
        let runs = schedule.next_n(ts("2026-07-15T00:00:00Z"), 3);
        assert_eq!(
            runs,
            vec![
                ts("2026-07-16T00:00:00Z"),
                ts("2026-07-18T00:00:00Z"),
                ts("2026-07-21T00:00:00Z"),
            ]
        );
    }
}
