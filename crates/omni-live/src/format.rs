//! Notification text helpers: date-fns `formatDistance` (en-US, no seconds,
//! no suffix) and `Number#toLocaleString()` digit grouping.

use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, Zoned};
use omni_core::js::math_round;

const MINUTES_IN_DAY: f64 = 1_440.0;
const MINUTES_IN_ALMOST_TWO_DAYS: f64 = 2_520.0;
const MINUTES_IN_MONTH: f64 = 43_200.0;

#[allow(clippy::cast_possible_truncation)]
fn as_int(value: f64) -> i64 {
    value as i64
}

/// `count.toLocaleString()` in the en-US default locale: `12345` -> `"12,345"`.
pub fn group_digits(count: i64) -> String {
    let digits = count.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if count < 0 {
        out.push('-');
    }
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `"<n> viewers"` with grouping.
pub fn format_count(count: i64) -> String {
    format!("{} viewers", group_digits(count))
}

fn zoned(ms: i64, tz: &TimeZone) -> Option<Zoned> {
    Timestamp::from_millisecond(ms)
        .ok()
        .map(|t| t.to_zoned(tz.clone()))
}

/// date-fns `getTimezoneOffsetInMilliseconds`: minus the UTC offset.
fn tz_offset_ms(ms: i64, tz: &TimeZone) -> i64 {
    Timestamp::from_millisecond(ms)
        .map(|t| -i64::from(tz.to_offset(t).seconds()) * 1_000)
        .unwrap_or(0)
}

/// date-fns `differenceInMonths(later, earlier)` for `later >= earlier`,
/// in the local time zone. Month arithmetic clamps the day of month (jiff)
/// where JS `setMonth` overflows into the next month; the difference only
/// matters at month-end boundaries of distances of two months or more.
fn difference_in_months(later_ms: i64, earlier_ms: i64, tz: &TimeZone) -> i64 {
    let (Some(later), Some(earlier)) = (zoned(later_ms, tz), zoned(earlier_ms, tz)) else {
        return 0;
    };
    let calendar = (i64::from(later.year()) - i64::from(earlier.year())) * 12
        + (i64::from(later.month()) - i64::from(earlier.month()));
    let difference = calendar.abs();
    if difference < 1 {
        return 0;
    }
    let shifted = later
        .checked_sub(difference.months())
        .unwrap_or(later.clone());
    let last_month_not_full = shifted.timestamp() < earlier.timestamp();
    difference - i64::from(last_month_not_full)
}

/// date-fns `formatDistance(a, b)` with the default en-US locale: symmetric in
/// its arguments, local-time-zone DST correction included.
pub fn format_distance(a_ms: i64, b_ms: i64, tz: &TimeZone) -> String {
    let (earlier, later) = if a_ms > b_ms {
        (b_ms, a_ms)
    } else {
        (a_ms, b_ms)
    };
    // differenceInSeconds truncates toward zero.
    #[allow(clippy::cast_precision_loss)]
    let seconds = ((later - earlier) / 1_000) as f64;
    #[allow(clippy::cast_precision_loss)]
    let offset_seconds = ((tz_offset_ms(later, tz) - tz_offset_ms(earlier, tz)) / 1_000) as f64;
    let minutes = math_round((seconds - offset_seconds) / 60.0);

    if minutes < 2.0 {
        return if minutes == 0.0 {
            "less than a minute".to_owned()
        } else {
            plural(as_int(minutes), "minute")
        };
    }
    if minutes < 45.0 {
        return plural(as_int(minutes), "minute");
    }
    if minutes < 90.0 {
        return "about 1 hour".to_owned();
    }
    if minutes < MINUTES_IN_DAY {
        let hours = as_int(math_round(minutes / 60.0));
        return format!("about {}", plural(hours, "hour"));
    }
    if minutes < MINUTES_IN_ALMOST_TWO_DAYS {
        return "1 day".to_owned();
    }
    if minutes < MINUTES_IN_MONTH {
        return plural(as_int(math_round(minutes / MINUTES_IN_DAY)), "day");
    }
    if minutes < MINUTES_IN_MONTH * 2.0 {
        let months = as_int(math_round(minutes / MINUTES_IN_MONTH));
        return format!("about {}", plural(months, "month"));
    }
    let months = difference_in_months(later, earlier, tz);
    if months < 12 {
        return plural(as_int(math_round(minutes / MINUTES_IN_MONTH)), "month");
    }
    let since_start_of_year = months % 12;
    let years = months / 12;
    if since_start_of_year < 3 {
        format!("about {}", plural(years, "year"))
    } else if since_start_of_year < 9 {
        format!("over {}", plural(years, "year"))
    } else {
        format!("almost {}", plural(years + 1, "year"))
    }
}

fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn vancouver() -> TimeZone {
        TimeZone::get("America/Vancouver").unwrap()
    }

    #[test]
    fn groups_digits_like_en_us() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(12_345_678), "12,345,678");
        assert_eq!(format_count(11_821), "11,821 viewers");
    }

    #[test]
    fn matches_date_fns_buckets() {
        let tz = TimeZone::UTC;
        let t0 = 1_780_000_000_000;
        let cases = [
            (20_000, "less than a minute"),
            (31_000, "1 minute"),
            (5 * MIN, "5 minutes"),
            (44 * MIN, "44 minutes"),
            (45 * MIN, "about 1 hour"),
            (89 * MIN, "about 1 hour"),
            (90 * MIN, "about 2 hours"),
            (10 * 60 * MIN, "about 10 hours"),
            (24 * 60 * MIN, "1 day"),
            (42 * 60 * MIN, "2 days"),
            (29 * 24 * 60 * MIN, "29 days"),
            (30 * 24 * 60 * MIN, "about 1 month"),
            (45 * 24 * 60 * MIN, "about 2 months"),
            (61 * 24 * 60 * MIN, "2 months"),
            (400 * 24 * 60 * MIN, "about 1 year"),
            (500 * 24 * 60 * MIN, "over 1 year"),
            (700 * 24 * 60 * MIN, "almost 2 years"),
        ];
        for (delta, expected) in cases {
            assert_eq!(format_distance(t0 + delta, t0, &tz), expected, "{delta}");
            assert_eq!(
                format_distance(t0, t0 + delta, &tz),
                expected,
                "{delta} swapped"
            );
        }
    }

    #[test]
    fn corrects_for_a_dst_change_between_the_dates() {
        // 2026-03-08 spring forward in Vancouver: 23 elapsed hours span one wall-clock day.
        let before = 1_772_956_800_000; // 2026-03-08T08:00:00Z (00:00 PST)
        let after = before + 23 * 60 * MIN;
        assert_eq!(format_distance(after, before, &vancouver()), "1 day");
        assert_eq!(
            format_distance(after, before, &TimeZone::UTC),
            "about 23 hours"
        );
    }
}
