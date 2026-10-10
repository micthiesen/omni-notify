//! Calendar-day labels from day indexes (days since 1970-01-01), so pages
//! can name dates without a timezone shifting them ("Today", "Thu, Oct 9").

use super::js::{civil_from_days, days_from_civil};

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The day index of a text starting `YYYY-MM-DD` (date or local datetime).
pub fn parse_ymd(text: &str) -> Option<i64> {
    let head = text.get(0..10)?;
    let mut parts = head.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day) && year > 1900)
        .then(|| days_from_civil(year, month, day))
}

/// `YYYY-MM-DD` of a day index.
pub fn ymd(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `Thu`.
pub fn weekday(day: i64) -> &'static str {
    WEEKDAYS[(day + 4).rem_euclid(7) as usize]
}

/// `Oct 9`, or `Oct 9, 2027` outside `today`'s year.
pub fn short_date(day: i64, today: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    let month = MONTHS[(m - 1).clamp(0, 11) as usize];
    if y == civil_from_days(today).0 {
        format!("{month} {d}")
    } else {
        format!("{month} {d}, {y}")
    }
}

/// `Today`, `Tomorrow`, `Yesterday`, otherwise `Thu, Oct 9`.
pub fn day_label(day: i64, today: i64) -> String {
    match day - today {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        -1 => "Yesterday".to_owned(),
        _ => format!("{}, {}", weekday(day), short_date(day, today)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_days_relative_to_today() {
        let today = parse_ymd("2026-10-09").unwrap();
        assert_eq!(weekday(today), "Fri");
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(today + 1, today), "Tomorrow");
        assert_eq!(day_label(today + 2, today), "Sun, Oct 11");
        assert_eq!(
            short_date(parse_ymd("2027-01-02").unwrap(), today),
            "Jan 2, 2027"
        );
        assert_eq!(parse_ymd("2026-10-09 00:00:00"), Some(today));
        assert_eq!(parse_ymd("--//--"), None);
        assert_eq!(ymd(today), "2026-10-09");
    }
}
