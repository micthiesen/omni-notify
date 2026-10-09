//! `new Date(header)` for Date headers. mailparser hands the header value to
//! the `Date` constructor, so this is V8's `Date.parse` (RFC 5322 with obsolete
//! forms, comments, named zones and two-digit years, plus ISO 8601), reading
//! zone-less dates as local time in the process zone. mailparser substitutes
//! "now" for anything unparseable, which the caller does.

use jiff::tz::TimeZone;

/// Epoch ms, or `None` where JS would produce an invalid Date.
pub(crate) fn parse_date(input: &str, tz: &TimeZone) -> Option<i64> {
    omni_core::js::date_parse(input, tz)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vancouver() -> TimeZone {
        TimeZone::get("America/Vancouver").expect("tzdb has America/Vancouver")
    }

    #[test]
    fn parses_rfc5322_dates() {
        let tz = vancouver();
        assert_eq!(
            parse_date("Tue, 01 Sep 2026 10:00:00 +0000", &tz),
            Some(1_788_256_800_000)
        );
        assert_eq!(
            parse_date("1 Sep 2026 06:00:00 -0400 (EDT)", &tz),
            Some(1_788_256_800_000)
        );
        assert_eq!(
            parse_date("Tue, 1 Sep 26 10:00 GMT", &tz),
            Some(1_788_256_800_000)
        );
        assert_eq!(
            parse_date("2026-09-01T10:00:00Z", &tz),
            Some(1_788_256_800_000)
        );
        assert_eq!(parse_date("not a date", &tz), None);
    }

    #[test]
    fn reads_zone_less_dates_as_local_time() {
        // node with TZ=America/Vancouver:
        // new Date("Tue, 01 Sep 2026 03:00:00").getTime() === 1788256800000
        assert_eq!(
            parse_date("Tue, 01 Sep 2026 03:00:00", &vancouver()),
            Some(1_788_256_800_000)
        );
        assert_eq!(
            parse_date("Tue, 01 Sep 2026 03:00:00", &TimeZone::UTC),
            Some(1_788_231_600_000)
        );
        // ISO date-only stays UTC; ISO date-time without offset is local.
        assert_eq!(
            parse_date("2026-09-01", &vancouver()),
            Some(1_788_220_800_000)
        );
        assert_eq!(
            parse_date("2026-09-01T03:00:00", &vancouver()),
            Some(1_788_256_800_000)
        );
    }
}
