//! `Intl` en-US formatting used in briefing prompts, and JS `String#replace`
//! semantics for the history placeholder.

use jiff::tz::TimeZone;
use jiff::{Zoned, civil};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

fn zoned(ms: i64, tz: &TimeZone) -> Zoned {
    omni_core::clock::timestamp_from_ms(ms).to_zoned(tz.clone())
}

fn month_name(date: civil::Date) -> &'static str {
    let index = usize::try_from(date.month() - 1).unwrap_or(0);
    MONTHS.get(index).copied().unwrap_or("January")
}

fn weekday_name(date: civil::Date) -> &'static str {
    let index = usize::try_from(date.weekday().to_monday_zero_offset()).unwrap_or(0);
    WEEKDAYS.get(index).copied().unwrap_or("Monday")
}

/// `{hour: "numeric", minute: "2-digit"}`: `2:30 PM`.
fn clock_time(z: &Zoned) -> String {
    let hour = z.hour();
    let (twelve, meridiem) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{twelve}:{:02} {meridiem}", z.minute())
}

/// `toLocaleDateString("en-US", {weekday: "long", year: "numeric", month: "long",
/// day: "numeric"})`: `Friday, February 6, 2026`.
pub fn format_long_date(ms: i64, tz: &TimeZone) -> String {
    let z = zoned(ms, tz);
    let date = z.date();
    format!(
        "{}, {} {}, {}",
        weekday_name(date),
        month_name(date),
        date.day(),
        date.year()
    )
}

/// `toLocaleTimeString("en-US", {hour: "numeric", minute: "2-digit",
/// timeZoneName: "short"})`: `2:30 PM PST`.
pub fn format_time_with_zone(ms: i64, tz: &TimeZone) -> String {
    let z = zoned(ms, tz);
    format!("{} {}", clock_time(&z), short_zone_name(&z))
}

/// `{month: "short", day: "numeric"}` and `{hour: "numeric", minute: "2-digit"}`
/// joined as the history list shows them: `Feb 6, 2:30 PM`.
pub fn format_month_day_time(ms: i64, tz: &TimeZone) -> String {
    let z = zoned(ms, tz);
    let month = month_name(z.date());
    format!("{} {}, {}", &month[..3], z.day(), clock_time(&z))
}

/// CLDR en-US short zone names: North American zones keep their
/// abbreviation (node prints `HAST`/`HADT` for America/Adak, where tzdb says
/// `HST`/`HDT`), UTC is `UTC`, everything else is `GMT±H[:MM]`.
fn short_zone_name(z: &Zoned) -> String {
    const US_ABBREVIATIONS: [&str; 15] = [
        "PST", "PDT", "MST", "MDT", "CST", "CDT", "EST", "EDT", "AKST", "AKDT", "HST", "HDT",
        "AST", "ADT", "UTC",
    ];
    let info = z.time_zone().to_offset_info(z.timestamp());
    let abbreviation = info.abbreviation();
    if US_ABBREVIATIONS.contains(&abbreviation) {
        return abbreviation.to_owned();
    }
    let seconds = info.offset().seconds();
    if seconds == 0 {
        return "GMT".to_owned();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let total_minutes = seconds.unsigned_abs() / 60;
    let (hours, minutes) = (total_minutes / 60, total_minutes % 60);
    if minutes == 0 {
        format!("GMT{sign}{hours}")
    } else {
        format!("GMT{sign}{hours}:{minutes:02}")
    }
}

/// `String#replace(pattern, replacement)` with a string pattern: replaces the
/// first occurrence and expands `$$`, `$&`, `` $` `` and `$'` in the replacement.
pub fn js_replace_first(haystack: &str, pattern: &str, replacement: &str) -> String {
    let Some(start) = haystack.find(pattern) else {
        return haystack.to_owned();
    };
    let end = start + pattern.len();
    let mut expanded = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            expanded.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                expanded.push('$');
                chars.next();
            }
            Some('&') => {
                expanded.push_str(pattern);
                chars.next();
            }
            Some('`') => {
                expanded.push_str(&haystack[..start]);
                chars.next();
            }
            Some('\'') => {
                expanded.push_str(&haystack[end..]);
                chars.next();
            }
            _ => expanded.push('$'),
        }
    }
    format!("{}{expanded}{}", &haystack[..start], &haystack[end..])
}

/// Epoch ms of a wall-clock time in `tz` (test and fixture helper).
pub fn local_ms(tz: &TimeZone, datetime: civil::DateTime) -> Option<i64> {
    datetime
        .to_zoned(tz.clone())
        .ok()
        .map(|z| z.timestamp().as_millisecond())
}

pub use omni_core::clock::log_timestamp;

#[cfg(test)]
mod tests {
    use super::*;

    fn vancouver() -> TimeZone {
        TimeZone::get("America/Vancouver").unwrap()
    }

    fn at(y: i16, mo: i8, d: i8, h: i8, mi: i8, tz: &TimeZone) -> i64 {
        local_ms(tz, civil::date(y, mo, d).at(h, mi, 0, 0)).unwrap()
    }

    #[test]
    fn formats_like_intl_en_us() {
        let tz = vancouver();
        let ms = at(2026, 2, 6, 14, 30, &tz);
        assert_eq!(format_long_date(ms, &tz), "Friday, February 6, 2026");
        assert_eq!(format_time_with_zone(ms, &tz), "2:30 PM PST");
        assert_eq!(format_month_day_time(ms, &tz), "Feb 6, 2:30 PM");
        let summer = at(2026, 7, 6, 0, 5, &tz);
        assert_eq!(format_time_with_zone(summer, &tz), "12:05 AM PDT");
        assert_eq!(
            format_month_day_time(at(2026, 7, 6, 12, 5, &tz), &tz),
            "Jul 6, 12:05 PM"
        );
    }

    #[test]
    fn non_us_zones_use_gmt_offsets() {
        let kolkata = TimeZone::get("Asia/Kolkata").unwrap();
        let ms = at(2026, 2, 6, 17, 40, &kolkata);
        assert_eq!(format_time_with_zone(ms, &kolkata), "5:40 PM GMT+5:30");
        let utc = TimeZone::UTC;
        assert_eq!(format_time_with_zone(0, &utc), "12:00 AM UTC");
        // Values printed by node 24 (en-US, `timeZoneName: "short"`).
        let at_utc = |zone: &str, month: i8| {
            let tz = TimeZone::get(zone).unwrap();
            let ms =
                local_ms(&TimeZone::UTC, civil::date(2026, month, 15).at(12, 0, 0, 0)).unwrap();
            format_time_with_zone(ms, &tz)
        };
        assert_eq!(at_utc("Europe/London", 1), "12:00 PM GMT");
        assert_eq!(at_utc("Europe/London", 7), "1:00 PM GMT+1");
        assert_eq!(at_utc("America/Halifax", 7), "9:00 AM ADT");
        assert_eq!(at_utc("America/St_Johns", 1), "8:30 AM GMT-3:30");
        assert_eq!(at_utc("Australia/Sydney", 1), "11:00 PM GMT+11");
    }

    #[test]
    fn replace_first_expands_dollar_patterns() {
        assert_eq!(js_replace_first("a X b X", "X", "y"), "a y b X");
        assert_eq!(
            js_replace_first("a X b", "X", "$$-$&-$`-$'"),
            "a $-X-a - b b"
        );
        assert_eq!(js_replace_first("a X", "X", "$1 $"), "a $1 $");
        assert_eq!(js_replace_first("abc", "X", "y"), "abc");
    }

    #[test]
    fn log_timestamps_are_filesystem_safe() {
        let tz = vancouver();
        assert_eq!(
            log_timestamp(at(2026, 3, 16, 14, 30, &tz) + 5_000, &tz),
            "2026-03-16T14-30-05"
        );
    }
}
