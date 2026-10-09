//! `new Date(header)` for Date headers: RFC 5322 dates (with obsolete forms,
//! comments, named zones and two-digit years, as V8's legacy parser accepts)
//! and ISO 8601. A date without a zone is read as UTC (V8 would use the
//! process zone); mailparser substitutes "now" for anything unparseable,
//! which the caller does.

fn month(name: &str) -> Option<i8> {
    let lower = name.to_ascii_lowercase();
    let key = lower.get(..3)?;
    Some(match key {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

fn zone_minutes(token: &str) -> Option<i32> {
    let upper = token.to_ascii_uppercase();
    let named = match upper.as_str() {
        "UT" | "UTC" | "GMT" | "Z" => Some(0),
        "EST" => Some(-300),
        "EDT" => Some(-240),
        "CST" => Some(-360),
        "CDT" => Some(-300),
        "MST" => Some(-420),
        "MDT" => Some(-360),
        "PST" => Some(-480),
        "PDT" => Some(-420),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let body = upper
        .strip_prefix("GMT")
        .or_else(|| upper.strip_prefix("UTC"))
        .unwrap_or(&upper);
    let (sign, digits) = match body.as_bytes().first() {
        Some(b'+') => (1, &body[1..]),
        Some(b'-') => (-1, &body[1..]),
        _ => return None,
    };
    let digits: String = digits.chars().filter(|c| *c != ':').collect();
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: i32 = digits.parse().ok()?;
    let (hours, minutes) = if digits.len() <= 2 {
        (value, 0)
    } else {
        (value / 100, value % 100)
    };
    Some(sign * (hours * 60 + minutes))
}

fn strip_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut depth = 0usize;
    for c in input.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn parse_time(token: &str) -> Option<(i8, i8, i8)> {
    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let hour: i8 = parts[0].parse().ok()?;
    let minute: i8 = parts[1].parse().ok()?;
    let second: i8 = match parts.get(2) {
        Some(s) => s.split('.').next()?.parse().ok()?,
        None => 0,
    };
    Some((hour, minute, second))
}

fn rfc5322(input: &str) -> Option<i64> {
    let cleaned = strip_comments(input).replace(',', " ");
    let mut day: Option<i8> = None;
    let mut mon: Option<i8> = None;
    let mut year: Option<i16> = None;
    let mut time: Option<(i8, i8, i8)> = None;
    let mut zone: Option<i32> = None;
    for token in cleaned.split_whitespace() {
        if time.is_none()
            && token.contains(':')
            && let Some(t) = parse_time(token)
        {
            time = Some(t);
            continue;
        }
        if let Some(z) = zone_minutes(token)
            && (time.is_some() || zone.is_none())
        {
            zone = Some(z);
            continue;
        }
        if let Some(m) = month(token)
            && mon.is_none()
            && token.chars().all(|c| c.is_ascii_alphabetic() || c == '.')
        {
            mon = Some(m);
            continue;
        }
        if token.bytes().all(|b| b.is_ascii_digit()) {
            let n: i32 = token.parse().ok()?;
            if day.is_none() && token.len() <= 2 && (mon.is_none() || year.is_none()) && n <= 31 {
                day = Some(i8::try_from(n).ok()?);
            } else if year.is_none() {
                let full = if token.len() <= 2 {
                    if n < 50 { 2000 + n } else { 1900 + n }
                } else {
                    n
                };
                year = Some(i16::try_from(full).ok()?);
            } else {
                return None;
            }
            continue;
        }
        // Day-of-week names and other words are ignored like V8 does.
        if token.chars().all(|c| c.is_ascii_alphabetic() || c == '.') {
            continue;
        }
        return None;
    }
    let (hour, minute, second) = time.unwrap_or((0, 0, 0));
    let date = jiff::civil::Date::new(year?, mon?, day?).ok()?;
    let datetime = date.at(hour, minute, second, 0);
    let offset = jiff::tz::Offset::from_seconds(zone.unwrap_or(0) * 60).ok()?;
    let ts = offset.to_timestamp(datetime).ok()?;
    Some(ts.as_millisecond())
}

/// Epoch ms, or `None` where JS would produce an invalid Date.
pub(crate) fn parse_date(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(ts) = trimmed.parse::<jiff::Timestamp>() {
        return Some(ts.as_millisecond());
    }
    if let Ok(date) = trimmed.parse::<jiff::civil::Date>() {
        // ISO date-only forms are UTC in JS.
        return jiff::tz::Offset::UTC
            .to_timestamp(date.at(0, 0, 0, 0))
            .ok()
            .map(|ts| ts.as_millisecond());
    }
    rfc5322(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc5322_dates() {
        assert_eq!(
            parse_date("Tue, 01 Sep 2026 10:00:00 +0000"),
            Some(1_788_256_800_000)
        );
        assert_eq!(
            parse_date("1 Sep 2026 06:00:00 -0400 (EDT)"),
            Some(1_788_256_800_000)
        );
        assert_eq!(
            parse_date("Tue, 1 Sep 26 10:00 GMT"),
            Some(1_788_256_800_000)
        );
        assert_eq!(parse_date("2026-09-01T10:00:00Z"), Some(1_788_256_800_000));
        assert_eq!(parse_date("not a date"), None);
    }
}
