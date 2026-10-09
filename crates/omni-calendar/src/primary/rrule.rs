//! RFC 5545 recurrence rules: parsing, canonical formatting, and expansion in
//! civil (wall-clock) time.
//!
//! Expansion covers DAILY, WEEKLY, MONTHLY and YEARLY rules with INTERVAL,
//! COUNT, UNTIL, BYDAY (with ordinals for MONTHLY/YEARLY), BYMONTHDAY,
//! BYMONTH, BYSETPOS and WKST. Sub-daily frequencies and BYYEARDAY, BYWEEKNO,
//! BYHOUR, BYMINUTE and BYSECOND are parsed and preserved but not expanded:
//! such a series yields only its first instance and is flagged unsupported.

use jiff::civil::{Date, DateTime, Weekday};
use jiff::{Timestamp, ToSpan as _};

use super::time::{format_date, format_utc, parse_date, parse_local};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freq {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Freq {
    fn parse(value: &str) -> Option<Self> {
        Some(match value.to_ascii_uppercase().as_str() {
            "SECONDLY" => Freq::Secondly,
            "MINUTELY" => Freq::Minutely,
            "HOURLY" => Freq::Hourly,
            "DAILY" => Freq::Daily,
            "WEEKLY" => Freq::Weekly,
            "MONTHLY" => Freq::Monthly,
            "YEARLY" => Freq::Yearly,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Freq::Secondly => "SECONDLY",
            Freq::Minutely => "MINUTELY",
            Freq::Hourly => "HOURLY",
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        }
    }
}

/// `UNTIL` as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Until {
    Date(Date),
    Utc(Timestamp),
    /// A local date-time (allowed only with floating DTSTART).
    Local(DateTime),
}

/// One BYDAY entry: optional ordinal plus weekday.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeekdayNum {
    pub ordinal: Option<i16>,
    pub weekday: Weekday,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RRule {
    pub freq: Freq,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<Until>,
    pub by_day: Vec<WeekdayNum>,
    pub by_month_day: Vec<i8>,
    pub by_month: Vec<i8>,
    pub by_set_pos: Vec<i16>,
    pub wkst: Option<Weekday>,
    /// Parts the expander does not implement, kept verbatim (`NAME=value`).
    pub unsupported: Vec<String>,
    /// Unknown parts kept verbatim.
    pub other: Vec<String>,
}

/// The result of expanding a rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expansion {
    /// Instances in order, the series start first.
    pub instances: Vec<DateTime>,
    /// The iteration cap stopped expansion before the series or window ended.
    pub truncated: bool,
    /// The rule uses parts the expander does not implement.
    pub unsupported: bool,
}

pub fn weekday_code(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
        Weekday::Sunday => "SU",
    }
}

pub fn parse_weekday(code: &str) -> Option<Weekday> {
    Some(match code.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Monday,
        "TU" => Weekday::Tuesday,
        "WE" => Weekday::Wednesday,
        "TH" => Weekday::Thursday,
        "FR" => Weekday::Friday,
        "SA" => Weekday::Saturday,
        "SU" => Weekday::Sunday,
        _ => return None,
    })
}

fn parse_list<T>(value: &str, parse: impl Fn(&str) -> Option<T>) -> Result<Vec<T>, String> {
    value
        .split(',')
        .map(|v| parse(v.trim()).ok_or_else(|| format!("invalid value {v:?}")))
        .collect()
}

impl RRule {
    pub fn new(freq: Freq) -> Self {
        Self {
            freq,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            wkst: None,
            unsupported: Vec::new(),
            other: Vec::new(),
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        let mut freq = None;
        let mut rule = RRule::new(Freq::Daily);
        for part in value.trim().split(';').filter(|p| !p.is_empty()) {
            let (name, v) = part
                .split_once('=')
                .ok_or_else(|| format!("malformed part {part:?}"))?;
            match name.to_ascii_uppercase().as_str() {
                "FREQ" => freq = Some(Freq::parse(v).ok_or_else(|| format!("bad FREQ {v}"))?),
                "INTERVAL" => {
                    rule.interval = v
                        .parse::<u32>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("bad INTERVAL")?;
                }
                "COUNT" => rule.count = Some(v.parse::<u32>().map_err(|_| "bad COUNT")?),
                "UNTIL" => {
                    rule.until = Some(if let Some(d) = parse_date(v) {
                        Until::Date(d)
                    } else if let Some(body) = v.strip_suffix(['Z', 'z']) {
                        let local = parse_local(body).ok_or("bad UNTIL")?;
                        Until::Utc(
                            local
                                .to_zoned(jiff::tz::TimeZone::UTC)
                                .map_err(|e| e.to_string())?
                                .timestamp(),
                        )
                    } else {
                        Until::Local(parse_local(v).ok_or("bad UNTIL")?)
                    });
                }
                "BYDAY" => {
                    rule.by_day = parse_list(v, |item| {
                        let split = item.len().checked_sub(2)?;
                        let weekday = parse_weekday(item.get(split..)?)?;
                        let prefix = item.get(..split)?;
                        let ordinal = if prefix.is_empty() {
                            None
                        } else {
                            let n: i16 = prefix.trim_start_matches('+').parse().ok()?;
                            if n == 0 || n.abs() > 53 {
                                return None;
                            }
                            Some(n)
                        };
                        Some(WeekdayNum { ordinal, weekday })
                    })?;
                }
                "BYMONTHDAY" => {
                    rule.by_month_day = parse_list(v, |item| {
                        let n: i8 = item.trim_start_matches('+').parse().ok()?;
                        (n != 0 && n.abs() <= 31).then_some(n)
                    })?;
                }
                "BYMONTH" => {
                    rule.by_month = parse_list(v, |item| {
                        let n: i8 = item.parse().ok()?;
                        (1..=12).contains(&n).then_some(n)
                    })?;
                }
                "BYSETPOS" => {
                    rule.by_set_pos = parse_list(v, |item| {
                        let n: i16 = item.trim_start_matches('+').parse().ok()?;
                        (n != 0 && n.abs() <= 366).then_some(n)
                    })?;
                }
                "WKST" => rule.wkst = Some(parse_weekday(v).ok_or("bad WKST")?),
                "BYYEARDAY" | "BYWEEKNO" | "BYHOUR" | "BYMINUTE" | "BYSECOND" => {
                    rule.unsupported.push(part.to_owned());
                }
                _ => rule.other.push(part.to_owned()),
            }
        }
        rule.freq = freq.ok_or("RRULE has no FREQ")?;
        Ok(rule)
    }

    /// Canonical text: FREQ, INTERVAL (when not 1), COUNT/UNTIL, BYMONTH,
    /// BYMONTHDAY, BYDAY, BYSETPOS, WKST, then preserved parts.
    pub fn to_value(&self) -> String {
        let mut parts = vec![format!("FREQ={}", self.freq.as_str())];
        if self.interval != 1 {
            parts.push(format!("INTERVAL={}", self.interval));
        }
        if let Some(count) = self.count {
            parts.push(format!("COUNT={count}"));
        }
        match &self.until {
            Some(Until::Date(d)) => parts.push(format!("UNTIL={}", format_date(*d))),
            Some(Until::Utc(ts)) => parts.push(format!("UNTIL={}", format_utc(*ts))),
            Some(Until::Local(dt)) => {
                parts.push(format!("UNTIL={}", super::time::format_local(*dt)))
            }
            None => {}
        }
        let join = |items: Vec<String>| items.join(",");
        if !self.by_month.is_empty() {
            parts.push(format!(
                "BYMONTH={}",
                join(self.by_month.iter().map(i8::to_string).collect())
            ));
        }
        if !self.by_month_day.is_empty() {
            parts.push(format!(
                "BYMONTHDAY={}",
                join(self.by_month_day.iter().map(i8::to_string).collect())
            ));
        }
        if !self.by_day.is_empty() {
            parts.push(format!(
                "BYDAY={}",
                join(
                    self.by_day
                        .iter()
                        .map(|d| format!(
                            "{}{}",
                            d.ordinal.map(|n| n.to_string()).unwrap_or_default(),
                            weekday_code(d.weekday)
                        ))
                        .collect()
                )
            ));
        }
        if !self.by_set_pos.is_empty() {
            parts.push(format!(
                "BYSETPOS={}",
                join(self.by_set_pos.iter().map(i16::to_string).collect())
            ));
        }
        if let Some(wkst) = self.wkst {
            parts.push(format!("WKST={}", weekday_code(wkst)));
        }
        parts.extend(self.unsupported.iter().cloned());
        parts.extend(self.other.iter().cloned());
        parts.join(";")
    }

    /// Whether the structured model can express and edit this rule.
    pub fn is_editable(&self) -> bool {
        self.unsupported.is_empty()
            && matches!(
                self.freq,
                Freq::Daily | Freq::Weekly | Freq::Monthly | Freq::Yearly
            )
    }

    pub fn is_expandable(&self) -> bool {
        self.is_editable()
    }

    /// Whether `instance` is at or before UNTIL; `to_instant` reads a civil
    /// instance in the series' zone (needed for a UTC UNTIL).
    pub fn until_allows(
        &self,
        instance: DateTime,
        to_instant: impl Fn(DateTime) -> Option<Timestamp>,
    ) -> bool {
        match &self.until {
            None => true,
            Some(Until::Date(d)) => instance.date() <= *d,
            Some(Until::Local(dt)) => instance <= *dt,
            Some(Until::Utc(limit)) => to_instant(instance).is_none_or(|ts| ts <= *limit),
        }
    }

    /// Expands from `start` (always the first instance) up to and including
    /// `stop_after`. `skip_to` lets a COUNT-less rule jump ahead to the period
    /// containing it. `cap` bounds the periods visited.
    pub fn expand(
        &self,
        start: DateTime,
        skip_to: Option<DateTime>,
        stop_after: DateTime,
        cap: usize,
        mut until_ok: impl FnMut(DateTime) -> bool,
    ) -> Expansion {
        let mut out = Expansion::default();
        if !self.is_expandable() {
            out.unsupported = true;
            if start <= stop_after {
                out.instances.push(start);
            }
            return out;
        }
        let mut emitted: u32 = 0;
        if start <= stop_after {
            out.instances.push(start);
        }
        emitted += 1;
        if self.count.is_some_and(|c| c <= 1) || !until_ok(start) {
            return out;
        }
        let interval = i64::from(self.interval);
        let mut period: i64 = match (self.count, skip_to) {
            (None, Some(target)) if target > start => {
                (self.periods_between(start.date(), target.date()) / interval - 1).max(0) * interval
            }
            _ => 0,
        };
        let mut visited = 0usize;
        loop {
            if visited >= cap {
                out.truncated = true;
                return out;
            }
            visited += 1;
            let Some(period_start) = self.period_start(start.date(), period) else {
                return out;
            };
            if period_start > stop_after.date() {
                return out;
            }
            for date in self.candidates(period_start, start.date()) {
                let instance = date.to_datetime(start.time());
                if instance <= start {
                    continue;
                }
                if !until_ok(instance) {
                    return out;
                }
                if instance > stop_after {
                    return out;
                }
                emitted += 1;
                out.instances.push(instance);
                if self.count.is_some_and(|c| emitted >= c) {
                    return out;
                }
            }
            period += interval;
        }
    }

    fn week_start(&self) -> Weekday {
        self.wkst.unwrap_or(Weekday::Monday)
    }

    fn week_of(&self, date: Date) -> Date {
        let back = i64::from(date.weekday().since(self.week_start()));
        date.checked_sub(back.days()).unwrap_or(date)
    }

    /// Whole periods from the period of `start` to the period of `target`.
    fn periods_between(&self, start: Date, target: Date) -> i64 {
        let days = |a: Date, b: Date| -> i64 {
            b.since(a)
                .map(|span| i64::from(span.get_days()))
                .unwrap_or(0)
        };
        match self.freq {
            Freq::Weekly => days(self.week_of(start), self.week_of(target)) / 7,
            Freq::Monthly => {
                (i64::from(target.year()) - i64::from(start.year())) * 12
                    + i64::from(target.month())
                    - i64::from(start.month())
            }
            Freq::Yearly => i64::from(target.year()) - i64::from(start.year()),
            _ => days(start, target),
        }
    }

    /// The first day of period number `n` after the start's period.
    fn period_start(&self, start: Date, n: i64) -> Option<Date> {
        match self.freq {
            Freq::Weekly => self.week_of(start).checked_add((n * 7).days()).ok(),
            Freq::Monthly => start.first_of_month().checked_add(n.months()).ok(),
            Freq::Yearly => start.first_of_year().checked_add(n.years()).ok(),
            _ => start.checked_add(n.days()).ok(),
        }
    }

    /// Candidate dates of one period (sorted, BYSETPOS applied).
    fn candidates(&self, period_start: Date, start: Date) -> Vec<Date> {
        let mut dates: Vec<Date> = match self.freq {
            Freq::Daily => {
                let d = period_start;
                let keep = (self.by_month.is_empty() || self.by_month.contains(&d.month()))
                    && (self.by_month_day.is_empty() || month_day_matches(&self.by_month_day, d))
                    && (self.by_day.is_empty()
                        || self.by_day.iter().any(|w| w.weekday == d.weekday()));
                if keep { vec![d] } else { Vec::new() }
            }
            Freq::Weekly => {
                let mut days: Vec<Date> = if self.by_day.is_empty() {
                    vec![day_in_week(
                        period_start,
                        start.weekday(),
                        self.week_start(),
                    )]
                } else {
                    self.by_day
                        .iter()
                        .map(|w| day_in_week(period_start, w.weekday, self.week_start()))
                        .collect()
                };
                days.retain(|d| self.by_month.is_empty() || self.by_month.contains(&d.month()));
                days
            }
            Freq::Monthly => {
                if !self.by_month.is_empty() && !self.by_month.contains(&period_start.month()) {
                    Vec::new()
                } else {
                    self.month_candidates(period_start, start)
                }
            }
            Freq::Yearly => self.year_candidates(period_start.year(), start),
            _ => Vec::new(),
        };
        dates.sort();
        dates.dedup();
        if self.by_set_pos.is_empty() {
            return dates;
        }
        let len = dates.len() as i64;
        let mut picked: Vec<Date> = self
            .by_set_pos
            .iter()
            .filter_map(|pos| {
                let index = if *pos > 0 {
                    i64::from(*pos) - 1
                } else {
                    len + i64::from(*pos)
                };
                usize::try_from(index)
                    .ok()
                    .and_then(|i| dates.get(i).copied())
            })
            .collect();
        picked.sort();
        picked.dedup();
        picked
    }

    fn month_candidates(&self, first: Date, start: Date) -> Vec<Date> {
        match (self.by_month_day.is_empty(), self.by_day.is_empty()) {
            (true, true) => first
                .with()
                .day(start.day())
                .build()
                .ok()
                .into_iter()
                .collect(),
            (false, true) => month_days(&self.by_month_day, first),
            (true, false) => weekdays_in(&self.by_day, first, first.last_of_month()),
            (false, false) => {
                let days = month_days(&self.by_month_day, first);
                let weekdays = weekdays_in(&self.by_day, first, first.last_of_month());
                days.into_iter().filter(|d| weekdays.contains(d)).collect()
            }
        }
    }

    fn year_candidates(&self, year: i16, start: Date) -> Vec<Date> {
        let Ok(jan1) = Date::new(year, 1, 1) else {
            return Vec::new();
        };
        let months: Vec<i8> = if !self.by_month.is_empty() {
            self.by_month.clone()
        } else if !self.by_month_day.is_empty() {
            (1..=12).collect()
        } else if !self.by_day.is_empty() {
            // BYDAY without BYMONTH is relative to the year.
            return weekdays_in(&self.by_day, jan1, jan1.last_of_year());
        } else {
            vec![start.month()]
        };
        let mut out = Vec::new();
        for month in months {
            let Ok(first) = Date::new(year, month, 1) else {
                continue;
            };
            match (self.by_month_day.is_empty(), self.by_day.is_empty()) {
                (true, true) => out.extend(first.with().day(start.day()).build().ok()),
                (false, true) => out.extend(month_days(&self.by_month_day, first)),
                (true, false) => {
                    out.extend(weekdays_in(&self.by_day, first, first.last_of_month()))
                }
                (false, false) => {
                    let weekdays = weekdays_in(&self.by_day, first, first.last_of_month());
                    out.extend(
                        month_days(&self.by_month_day, first)
                            .into_iter()
                            .filter(|d| weekdays.contains(d)),
                    );
                }
            }
        }
        out
    }
}

fn month_day_matches(days: &[i8], date: Date) -> bool {
    let len = date.days_in_month();
    days.iter().any(|d| {
        let resolved = if *d > 0 { *d } else { len + 1 + *d };
        resolved == date.day()
    })
}

fn month_days(days: &[i8], first: Date) -> Vec<Date> {
    let len = first.days_in_month();
    days.iter()
        .filter_map(|d| {
            let day = if *d > 0 { *d } else { len + 1 + *d };
            (1..=len)
                .contains(&day)
                .then(|| first.with().day(day).build().ok())
                .flatten()
        })
        .collect()
}

/// BYDAY dates within `[from, to]`: every matching weekday, or the nth
/// (negative from the end) when an ordinal is given.
fn weekdays_in(by_day: &[WeekdayNum], from: Date, to: Date) -> Vec<Date> {
    let mut out = Vec::new();
    for spec in by_day {
        let first = from
            .checked_add(i64::from(spec.weekday.since(from.weekday())).days())
            .ok();
        let mut all = Vec::new();
        let mut cursor = first;
        while let Some(d) = cursor {
            if d > to {
                break;
            }
            all.push(d);
            cursor = d.checked_add(7.days()).ok();
        }
        match spec.ordinal {
            None => out.extend(all),
            Some(n) => {
                let index = if n > 0 {
                    i64::from(n) - 1
                } else {
                    all.len() as i64 + i64::from(n)
                };
                if let Some(d) = usize::try_from(index).ok().and_then(|i| all.get(i)) {
                    out.push(*d);
                }
            }
        }
    }
    out
}

fn day_in_week(week_start: Date, weekday: Weekday, wkst: Weekday) -> Date {
    let offset = i64::from(weekday.since(wkst));
    week_start.checked_add(offset.days()).unwrap_or(week_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime {
        s.parse().unwrap()
    }

    fn expand(rule: &str, start: &str, stop: &str) -> Vec<String> {
        let rule = RRule::parse(rule).unwrap();
        rule.expand(dt(start), None, dt(stop), 10_000, |i| {
            rule.until_allows(i, |d| {
                Some(d.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp())
            })
        })
        .instances
        .into_iter()
        .map(|d| d.to_string())
        .collect()
    }

    #[test]
    fn weekly_by_day_with_count() {
        assert_eq!(
            expand(
                "FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4",
                "2026-07-06T09:00",
                "2027-01-01T00:00"
            ),
            [
                "2026-07-06T09:00:00",
                "2026-07-08T09:00:00",
                "2026-07-13T09:00:00",
                "2026-07-15T09:00:00"
            ]
        );
    }

    #[test]
    fn monthly_second_tuesday_and_last_day() {
        assert_eq!(
            expand(
                "FREQ=MONTHLY;BYDAY=2TU;COUNT=3",
                "2026-01-13T18:00",
                "2027-01-01T00:00"
            ),
            [
                "2026-01-13T18:00:00",
                "2026-02-10T18:00:00",
                "2026-03-10T18:00:00"
            ]
        );
        assert_eq!(
            expand(
                "FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3",
                "2026-01-31T08:00",
                "2027-01-01T00:00"
            ),
            [
                "2026-01-31T08:00:00",
                "2026-02-28T08:00:00",
                "2026-03-31T08:00:00"
            ]
        );
    }

    #[test]
    fn monthly_on_the_31st_skips_short_months() {
        assert_eq!(
            expand(
                "FREQ=MONTHLY;COUNT=3",
                "2026-01-31T08:00",
                "2027-01-01T00:00"
            ),
            [
                "2026-01-31T08:00:00",
                "2026-03-31T08:00:00",
                "2026-05-31T08:00:00"
            ]
        );
    }

    #[test]
    fn yearly_and_until_date() {
        assert_eq!(
            expand(
                "FREQ=YEARLY;UNTIL=20280229",
                "2024-02-29T00:00",
                "2040-01-01T00:00"
            ),
            ["2024-02-29T00:00:00", "2028-02-29T00:00:00"]
        );
    }

    #[test]
    fn last_weekday_of_month_by_set_pos() {
        assert_eq!(
            expand(
                "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=2",
                "2026-07-31T17:00",
                "2027-01-01T00:00"
            ),
            ["2026-07-31T17:00:00", "2026-08-31T17:00:00"]
        );
    }

    #[test]
    fn skip_ahead_matches_full_expansion() {
        let rule = RRule::parse("FREQ=DAILY;INTERVAL=3").unwrap();
        let start = dt("2020-01-01T10:00");
        let full = rule.expand(start, None, dt("2026-07-10T00:00"), 10_000, |_| true);
        let skipped = rule.expand(
            start,
            Some(dt("2026-07-01T00:00")),
            dt("2026-07-10T00:00"),
            50,
            |_| true,
        );
        let tail: Vec<_> = full
            .instances
            .iter()
            .filter(|d| **d >= dt("2026-07-01T00:00"))
            .collect();
        let skipped_tail: Vec<_> = skipped
            .instances
            .iter()
            .filter(|d| **d >= dt("2026-07-01T00:00"))
            .collect();
        assert_eq!(tail, skipped_tail);
        assert!(!skipped.truncated);
    }

    #[test]
    fn canonical_text_round_trips() {
        let rule = RRule::parse("BYDAY=1SU;FREQ=YEARLY;BYMONTH=11;X-FOO=1").unwrap();
        assert_eq!(rule.to_value(), "FREQ=YEARLY;BYMONTH=11;BYDAY=1SU;X-FOO=1");
        let hourly = RRule::parse("FREQ=HOURLY;BYHOUR=9").unwrap();
        assert!(!hourly.is_editable());
    }
}
