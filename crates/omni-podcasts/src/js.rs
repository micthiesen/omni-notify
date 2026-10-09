//! JS-compatible helpers this package uses: `Date.parse` and `Number#toFixed`
//! (re-exported from `omni_core::js`) and date stamps.

use jiff::tz::TimeZone;

/// `new Date(ms).toISOString().slice(0, 10)` (UTC `YYYY-MM-DD`).
pub fn to_date_stamp(ms: i64) -> String {
    let iso = omni_core::js::to_iso_string(ms);
    iso.get(..10).unwrap_or(&iso).to_owned()
}

/// mitools `logTimestamp`: local `YYYY-MM-DDTHH-mm-ss`.
pub fn log_timestamp(ms: i64, tz: &TimeZone) -> String {
    let zoned = omni_core::clock::timestamp_from_ms(ms).to_zoned(tz.clone());
    zoned.strftime("%Y-%m-%dT%H-%M-%S").to_string()
}

/// `Date.parse` for feed and Castro dates (`omni_core::js::date_parse`): zone-less
/// results are read in `tz`; `None` stands for `NaN`.
pub use omni_core::js::date_parse as parse_date;
/// `Number#toFixed(digits)`.
pub use omni_core::js::to_fixed;

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> Option<i64> {
        parse_date(s, &TimeZone::UTC)
    }

    #[test]
    fn parses_feed_and_castro_dates() {
        assert_eq!(
            utc("Thu, 02 Jan 2025 12:00:00 GMT"),
            Some(1_735_819_200_000)
        );
        assert_eq!(
            utc("Fri, 02 Jan 2025 12:00:00 GMT"),
            Some(1_735_819_200_000)
        );
        assert_eq!(utc("2 Jan 2025 12:00:00 +0000"), Some(1_735_819_200_000));
        assert_eq!(
            utc("Thu, 02 Jan 2025 07:00:00 EST"),
            Some(1_735_819_200_000)
        );
        assert_eq!(
            utc("Thu, 02 Jan 2025 12:00:00 UTC"),
            Some(1_735_819_200_000)
        );
        assert_eq!(utc("2026-07-16T17:30:00.000Z"), Some(1_784_223_000_000));
        assert_eq!(utc("2025-01-02"), Some(1_735_776_000_000));
        assert_eq!(
            utc("  Thu,  02 Jan 2025 12:00:00 GMT "),
            Some(1_735_819_200_000)
        );
        assert_eq!(utc("not-a-real-date"), None);
        assert_eq!(utc(""), None);
    }

    #[test]
    fn to_fixed_matches_js() {
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(0.5, 0), "1");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(0.92, 3), "0.920");
        assert_eq!(to_fixed(1.0, 3), "1.000");
        assert_eq!(to_fixed(0.5281117584929783, 3), "0.528");
        assert_eq!(to_fixed(30.000000000000004, 0), "30");
        assert_eq!(to_fixed(-0.125, 2), "-0.13");
        assert_eq!(to_fixed(99.5, 0), "100");
    }

    #[test]
    fn date_stamp_is_utc_day() {
        assert_eq!(to_date_stamp(1_784_091_600_000), "2026-07-15");
    }
}
