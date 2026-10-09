//! Shared push presentation (`src/reset-alerts/presentation.ts`).

use jiff::tz::TimeZone;
use omni_core::js::{utf16_len, utf16_slice};

use crate::js::{is_js_whitespace, trim, trim_end};

/// Only signals published within this window are eligible, including on startup.
pub const ALERT_LOOKBACK_MS: i64 = 48 * 60 * 60_000;
/// Tolerated clock skew for timestamps slightly in the future.
pub const CLOCK_SKEW_MS: i64 = 5 * 60_000;

const PACIFIC: &str = "America/Vancouver";
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Intl.DateTimeFormat("en-CA", {timeZone: "America/Vancouver", month: "short",
/// day: "numeric", hour: "numeric", minute: "2-digit", timeZoneName: "short"})`:
/// `Oct 2, 7:00 a.m. PDT`. `ms` must be a valid JS date.
pub fn pacific_time(ms: i64) -> String {
    let tz = TimeZone::get(PACIFIC).unwrap_or(TimeZone::UTC);
    let ts = omni_core::clock::timestamp_from_ms(ms);
    let zoned = ts.to_zoned(tz.clone());
    let month = MONTHS
        .get(usize::try_from(zoned.month() - 1).unwrap_or(0))
        .copied()
        .unwrap_or("");
    let hour24 = zoned.hour();
    let hour12 = match hour24 % 12 {
        0 => 12,
        h => h,
    };
    let meridiem = if hour24 < 12 { "a.m." } else { "p.m." };
    // ICU names the zone by its CLDR metazone: UTC-7 is "PDT" even under BC's
    // permanent daylight time (tzdb abbreviates that as "MST"), UTC-8 is "PST".
    let abbreviation = match zoned.offset().seconds() {
        -25_200 => "PDT".to_owned(),
        -28_800 => "PST".to_owned(),
        _ => tz.to_offset_info(ts).abbreviation().to_owned(),
    };
    format!(
        "{month} {}, {hour12}:{:02} {meridiem} {abbreviation}",
        zoned.day(),
        zoned.minute(),
    )
}

/// Removes `http://` / `https://` runs up to the next JS whitespace
/// (`/https?:\/\/\S+/g`).
fn strip_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let next = [rest.find("http://"), rest.find("https://")]
            .into_iter()
            .flatten()
            .min();
        let Some(start) = next else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let scheme_len = if rest[start..].starts_with("https://") {
            8
        } else {
            7
        };
        let after = &rest[start + scheme_len..];
        let url_len = after.find(is_js_whitespace).unwrap_or(after.len());
        if url_len == 0 {
            // `\S+` needs at least one character after the scheme.
            out.push_str(&rest[start..start + scheme_len]);
        }
        rest = &after[url_len..];
    }
    out
}

/// Collapses JS whitespace runs into single spaces (`/\s+/g`).
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if is_js_whitespace(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Push previews carry the news, not a clipped reply thread or raw URLs:
/// URLs removed, whitespace collapsed, at most 200 UTF-16 units with an
/// ellipsis at a word boundary when one is near the end.
pub fn compact_summary(text: &str) -> String {
    let clean = trim(&collapse_whitespace(&strip_urls(text))).to_owned();
    if utf16_len(&clean) <= 200 {
        return clean;
    }
    let prefix = utf16_slice(&clean, 0, 197).into_owned();
    let space = prefix
        .rfind(' ')
        .map(|byte| utf16_len(&prefix[..byte]))
        .filter(|units| *units > 150);
    let cut = space.unwrap_or(197);
    format!("{}…", trim_end(&utf16_slice(&prefix, 0, cut)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_pacific_time_like_intl_en_ca() {
        let at = |s: &str| jiff::Timestamp::from_str_ms(s);
        assert_eq!(
            pacific_time(at("2026-10-02T14:00:00Z")),
            "Oct 2, 7:00 a.m. PDT"
        );
        assert_eq!(
            pacific_time(at("2026-10-02T07:00:00Z")),
            "Oct 2, 12:00 a.m. PDT"
        );
        assert_eq!(
            pacific_time(at("2026-10-02T19:05:00Z")),
            "Oct 2, 12:05 p.m. PDT"
        );
        assert_eq!(
            pacific_time(at("2026-05-09T23:59:00Z")),
            "May 9, 4:59 p.m. PDT"
        );
    }

    trait FromStrMs {
        fn from_str_ms(s: &str) -> i64;
    }
    impl FromStrMs for jiff::Timestamp {
        fn from_str_ms(s: &str) -> i64 {
            s.parse::<jiff::Timestamp>().unwrap().as_millisecond()
        }
    }

    #[test]
    fn compacts_urls_and_whitespace() {
        assert_eq!(
            compact_summary("  85% chance\n within https://t.co/x  24 hours. https://t. "),
            "85% chance within 24 hours."
        );
        let long = "word ".repeat(100);
        let compact = compact_summary(&long);
        assert!(utf16_len(&compact) <= 200);
        assert!(compact.ends_with("word…"));
        let solid = "A".repeat(400);
        assert_eq!(compact_summary(&solid), format!("{}…", "A".repeat(197)));
    }
}
