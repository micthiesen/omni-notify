//! `new Date(string)` for the date strings PressPods meets: ISO 8601 (a bare
//! date is UTC, a date-time without offset is local), RFC 2822, X's
//! `"Tue Jul 14 14:04:31 +0000 2026"`, JS `Date#toString` output, and
//! `"July 14, 2026"`. Unparseable input is `None` (an invalid Date).

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

fn local_ms(dt: DateTime, tz: &TimeZone) -> Option<i64> {
    dt.to_zoned(tz.clone())
        .ok()
        .map(|z| z.timestamp().as_millisecond())
}

/// Epoch milliseconds, or `None` for an invalid date.
pub fn parse_js_date(input: &str, tz: &TimeZone) -> Option<i64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(ts) = s.parse::<Timestamp>() {
        return Some(ts.as_millisecond());
    }
    if let Ok(date) = s.parse::<Date>()
        && s.len() <= 10
    {
        return date
            .to_zoned(TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp().as_millisecond());
    }
    if let Ok(dt) = s.parse::<DateTime>() {
        return local_ms(dt, tz);
    }
    if let Ok(zoned) = jiff::fmt::rfc2822::parse(s) {
        return Some(zoned.timestamp().as_millisecond());
    }
    let without_zone_name = match s.find(" (") {
        Some(i) if s.ends_with(')') => &s[..i],
        _ => s,
    };
    for format in ["%a %b %d %H:%M:%S %z %Y", "%a %b %d %Y %H:%M:%S GMT%z"] {
        if let Ok(zoned) = Zoned::strptime(format, without_zone_name) {
            return Some(zoned.timestamp().as_millisecond());
        }
    }
    for format in ["%B %d, %Y", "%b %d, %Y", "%d %B %Y", "%B %d %Y"] {
        if let Ok(date) = Date::strptime(format, s) {
            return local_ms(date.to_datetime(jiff::civil::Time::midnight()), tz);
        }
    }
    None
}

/// JS `Date#toString`-style rendering for model prompts.
pub fn js_date_string(ms: i64, tz: &TimeZone) -> String {
    match Timestamp::from_millisecond(ms) {
        Ok(ts) => ts
            .to_zoned(tz.clone())
            .strftime("%a %b %d %Y %H:%M:%S GMT%z")
            .to_string(),
        Err(_) => "Invalid Date".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> TimeZone {
        TimeZone::get("America/Vancouver").unwrap()
    }

    #[test]
    fn parses_the_formats_js_accepts() {
        let tz = tz();
        assert_eq!(
            parse_js_date("2026-07-14T14:04:31.000Z", &tz),
            Some(1_784_037_871_000)
        );
        assert_eq!(parse_js_date("2026-07-14", &tz), Some(1_783_987_200_000));
        assert_eq!(
            parse_js_date("Tue Jul 14 14:04:31 +0000 2026", &tz),
            Some(1_784_037_871_000)
        );
        assert_eq!(
            parse_js_date("Tue, 14 Jul 2026 14:04:31 GMT", &tz),
            Some(1_784_037_871_000)
        );
        assert_eq!(
            parse_js_date(
                "Tue Jul 14 2026 07:04:31 GMT-0700 (Pacific Daylight Time)",
                &tz
            ),
            Some(1_784_037_871_000)
        );
        assert_eq!(
            parse_js_date("2026-07-14T07:04:31", &tz),
            Some(1_784_037_871_000)
        );
        assert!(parse_js_date("July 14, 2026", &tz).is_some());
        assert_eq!(parse_js_date("not a date", &tz), None);
        assert_eq!(parse_js_date("", &tz), None);
    }
}
