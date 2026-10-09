//! `Date.parse` semantics: every expectation was produced by node (V8) with
//! `TZ=America/Vancouver`, covering the RFC 2822 and loose forms podcast feeds
//! use, V8's legacy-parser quirks, and DST gaps/overlaps for zone-less input.
#![allow(clippy::unwrap_used, clippy::unreadable_literal)]

use jiff::tz::TimeZone;
use omni_core::js::date_parse;

#[test]
fn matches_node_date_parse_for_feed_date_shapes() {
    let tz = TimeZone::get("America/Vancouver").unwrap();
    let cases: &[(&str, Option<i64>)] = &[
        ("Thu, 02 Jan 2025 12:00:00 GMT", Some(1_735_819_200_000)),
        ("Fri, 02 Jan 2025 12:00:00 GMT", Some(1_735_819_200_000)),
        ("Thu, 02 Jan 2025 07:00:00 EST", Some(1_735_819_200_000)),
        ("Thu, 02 Jan 2025 12:00:00 UTC", Some(1_735_819_200_000)),
        ("2026-07-16T17:30:00.000Z", Some(1_784_223_000_000)),
        ("2025-01-02", Some(1_735_776_000_000)),
        ("Wed, 31 Dec 2025 23:59:59 +1400", Some(1_767_175_199_000)),
        ("Mon, 03 Mar 2025 12:00:00 +05:30", Some(1_740_983_400_000)),
        ("Mon, 03 Mar 2025 12:00:00 GMT-5", Some(1_741_021_200_000)),
        ("Mon, 03 Mar 2025 12:00:00 -05", Some(1_741_021_200_000)),
        ("Mon, 03 Mar 2025 12:00:00.123 GMT", Some(1_741_003_200_123)),
        ("Mon, 03 Mar 2025 12:00:00.5Z", Some(1_741_003_200_500)),
        ("Mon, 03 Mar 2025 24:00:00 GMT", Some(1_741_046_400_000)),
        ("Mon, 03 Mar 2025 25:00:00 GMT", None),
        ("Mon, 30 Feb 2025 12:00:00 GMT", Some(1_740_916_800_000)),
        ("Mon, 32 Mar 2025 12:00:00 GMT", None),
        ("3/14/2025", Some(1_741_935_600_000)),
        ("3/14/2025 2:30 PM", Some(1_741_987_800_000)),
        ("2025/03/14 14:30:00", Some(1_741_987_800_000)),
        ("14 March 2025", Some(1_741_935_600_000)),
        ("March 14", Some(984_556_800_000)),
        ("Mon Mar 14 2025", Some(1_741_935_600_000)),
        ("12:00 Mar 14 2025", Some(1_741_978_800_000)),
        ("Mon, 03 Mar 2025 12:00:00 GMT foo", None),
        ("Mon, 03 Mar 2025 12:00:00 Z", Some(1_741_003_200_000)),
        ("Mon, 03 Mar 2025 12:00:00 z", Some(1_741_003_200_000)),
        ("Mon, 3 Mar 2025 12:00 pm GMT", Some(1_741_003_200_000)),
        ("Mon, 3 Mar 2025 00:00 am", Some(1_740_988_800_000)),
        ("Monday 3rd March 2025", None),
        ("2025-03-03T12:00:00.000+0000", Some(1_741_003_200_000)),
        ("2025-03-03 12:00", Some(1_741_032_000_000)),
        ("Mon, 03 Mar 99 12:00:00 GMT", Some(920_462_400_000)),
        ("Mon, 03 Mar 50 12:00:00 GMT", Some(-625_838_400_000)),
        ("Mon, 03 Mar 49 12:00:00 GMT", Some(2_498_385_600_000)),
        ("Mon, 03-Mar-2025 12:00:00 GMT", Some(1_741_003_200_000)),
        ("03-Mar-2025", Some(1_740_988_800_000)),
        ("Mar-03-2025", Some(1_740_988_800_000)),
        (
            "Mon, 03 Mar 2025 12:00:00 +0000 GMT",
            Some(1_741_003_200_000),
        ),
        (
            "Mon, 03 Mar 2025 12:00:00 GMT +0100",
            Some(1_740_999_600_000),
        ),
        ("1741003200000", None),
        ("Mon, 09 Mar 2025 02:30:00", Some(1_741_516_200_000)),
        ("Sun, 02 Nov 2025 01:30:00", Some(1_762_072_200_000)),
        ("Tue, 1 Jul 2025 10:00:00 GMT", Some(1751364000000)),
        ("Tue, 01 Jul 2025 10:00 GMT", Some(1751364000000)),
        ("Tue, 01 July 2025 10:00:00 GMT", Some(1751364000000)),
        ("Tuesday, 01 Jul 2025 10:00:00 GMT", Some(1751364000000)),
        ("Tue, 01 Jul 2025 10:00:00 PDT", Some(1751389200000)),
        ("Tue, 01 Jul 2025 10:00:00 +0000 (UTC)", Some(1751364000000)),
        ("Tue, 01 Jul 2025 10:00:00 CEST", None),
        ("Tue, 01 Jul 2025 10:00:00 BST", None),
        ("Tue, 01 Jul 2025 10:00:00 -0700", Some(1751389200000)),
        ("Tue, 01 Jul 2025 10:00:00", Some(1751389200000)),
        ("Tue, 01 Jul 25 10:00:00 GMT", Some(1751364000000)),
        ("Tue, 01 Jul 2025 10:00:00 GMT+0000", Some(1751364000000)),
        ("2025-07-01 10:00:00 +0000", Some(1751364000000)),
        ("Tue, 01 Jul 2025 1:00:00 GMT", Some(1751331600000)),
        ("Tue,01 Jul 2025 10:00:00 GMT", Some(1751364000000)),
        ("Jul 1, 2025", Some(1751353200000)),
        ("July 1, 2025 10:00 AM", Some(1751389200000)),
        (
            "Tue Jul 01 2025 10:00:00 GMT+0000 (Coordinated Universal Time)",
            Some(1751364000000),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(date_parse(input, &tz), *expected, "{input}");
    }
}
