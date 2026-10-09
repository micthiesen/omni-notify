//! JS semantics this crate needs beyond `omni_core::js`: `Date.parse` for the
//! ISO forms the sources use, `Math.round`, JS whitespace and `trim`, and
//! JSON numbers serialized the way `JSON.stringify` writes them.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use serde_json::Value;

/// JS `Date` range limit (`TimeClip`): |ms| above this is an invalid date.
pub const MAX_DATE_MS: i64 = 8_640_000_000_000_000;

/// `Date.parse` for ISO 8601 strings as V8 accepts them: date-only forms are
/// UTC, date-times without an offset are local time in `tz`, a `T` or a space
/// separates date and time, fractions of a second are truncated to
/// milliseconds and days past the month end roll over (V8 parity). Other
/// legacy formats V8 also accepts (`Oct 2 2026`) return `None`.
pub fn parse_date(input: &str, tz: &TimeZone) -> Option<i64> {
    let parts = IsoParts::parse(input)?;
    let first = Date::new(parts.year, parts.month, 1).ok()?;
    let date = first
        .checked_add(Span::new().days(i64::from(parts.day) - 1))
        .ok()?;
    let Some(time) = parts.time else {
        let ts = date.to_zoned(TimeZone::UTC).ok()?.timestamp();
        return clip(ts.as_millisecond());
    };
    let (hour, rolled) = if time.hour == 24 {
        if time.minute != 0 || time.second != 0 || time.millis != 0 {
            return None;
        }
        (0, true)
    } else {
        (time.hour, false)
    };
    let clock = Time::new(hour, time.minute, time.second, 0).ok()?;
    let mut civil = DateTime::from_parts(date, clock);
    if rolled {
        civil = civil.checked_add(Span::new().days(1)).ok()?;
    }
    let base_ms = match time.offset_minutes {
        Some(offset) => {
            let utc = civil
                .to_zoned(TimeZone::UTC)
                .ok()?
                .timestamp()
                .as_millisecond();
            utc - i64::from(offset) * 60_000
        }
        None => tz
            .to_ambiguous_timestamp(civil)
            .compatible()
            .ok()?
            .as_millisecond(),
    };
    clip(base_ms + i64::from(time.millis))
}

fn clip(ms: i64) -> Option<i64> {
    (ms.abs() <= MAX_DATE_MS).then_some(ms)
}

struct IsoTime {
    hour: i8,
    minute: i8,
    second: i8,
    millis: i32,
    offset_minutes: Option<i32>,
}

struct IsoParts {
    year: i16,
    month: i8,
    day: i8,
    time: Option<IsoTime>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn digits(&mut self, n: usize) -> Option<u32> {
        let slice = self.bytes.get(self.at..self.at + n)?;
        let mut value = 0u32;
        for b in slice {
            if !b.is_ascii_digit() {
                return None;
            }
            value = value * 10 + u32::from(b - b'0');
        }
        self.at += n;
        Some(value)
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.bytes.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

impl IsoParts {
    fn parse(input: &str) -> Option<Self> {
        let mut c = Cursor {
            bytes: input.as_bytes(),
            at: 0,
        };
        // `±YYYYYY` expanded years (`-000000` is invalid), else four digits.
        let year = match c.peek() {
            Some(sign @ (b'+' | b'-')) => {
                c.at += 1;
                let value = i64::from(c.digits(6)?);
                if sign == b'-' && value == 0 {
                    return None;
                }
                i16::try_from(if sign == b'-' { -value } else { value }).ok()?
            }
            _ => i16::try_from(c.digits(4)?).ok()?,
        };
        let mut month = 1;
        let mut day = 1;
        if c.eat(b'-') {
            month = c.digits(2)?;
            if c.eat(b'-') {
                day = c.digits(2)?;
            }
        }
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let month = i8::try_from(month).ok()?;
        let day = i8::try_from(day).ok()?;
        if c.done() {
            return Some(Self {
                year,
                month,
                day,
                time: None,
            });
        }
        if !(c.eat(b'T') || c.eat(b't') || c.eat(b' ')) {
            return None;
        }
        let hour = c.digits(2)?;
        if !c.eat(b':') {
            return None;
        }
        let minute = c.digits(2)?;
        let mut second = 0;
        let mut millis = 0;
        if c.eat(b':') {
            second = c.digits(2)?;
            if c.eat(b'.') || c.eat(b',') {
                let start = c.at;
                while c.peek().is_some_and(|b| b.is_ascii_digit()) {
                    c.at += 1;
                }
                let fraction = input.get(start..c.at)?;
                if fraction.is_empty() {
                    return None;
                }
                let padded: String = fraction.chars().chain("000".chars()).take(3).collect();
                millis = padded.parse::<i32>().ok()?;
            }
        }
        if hour > 24 || minute > 59 || second > 59 {
            return None;
        }
        let offset_minutes = if c.eat(b'Z') || c.eat(b'z') {
            Some(0)
        } else if let Some(sign @ (b'+' | b'-')) = c.peek() {
            c.at += 1;
            let oh = c.digits(2)?;
            // V8 also accepts `+HHMM` through its legacy parser.
            c.eat(b':');
            let om = c.digits(2)?;
            if oh > 23 || om > 59 {
                return None;
            }
            let total = i32::try_from(oh * 60 + om).ok()?;
            Some(if sign == b'-' { -total } else { total })
        } else {
            None
        };
        if !c.done() {
            return None;
        }
        Some(Self {
            year,
            month,
            day,
            time: Some(IsoTime {
                hour: i8::try_from(hour).ok()?,
                minute: i8::try_from(minute).ok()?,
                second: i8::try_from(second).ok()?,
                millis,
                offset_minutes,
            }),
        })
    }
}

/// `Math.round`: nearest integer, ties toward positive infinity.
pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// JS `\s` / `String#trim` whitespace (WhiteSpace and LineTerminator).
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String#trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `String#trimEnd`.
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

/// A JSON number as `JSON.stringify` would write it: integral values within
/// the safe range become integers (`12`, not `12.0`).
pub fn number(n: f64) -> Value {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if n.is_finite() && n.fract() == 0.0 && n.abs() <= MAX_SAFE {
        #[allow(clippy::cast_possible_truncation)]
        let int = n as i64;
        Value::from(int)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

/// Rewrites every integral float in `value` as an integer (see [`number`]).
pub fn normalize_numbers(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => n.as_f64().map_or(Value::Number(n), number),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, normalize_numbers(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Epoch milliseconds for a timestamp.
pub fn timestamp_ms(ts: Timestamp) -> i64 {
    ts.as_millisecond()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> TimeZone {
        TimeZone::get("America/Vancouver").unwrap()
    }

    #[test]
    fn parses_iso_forms_like_v8() {
        let tz = tz();
        assert_eq!(parse_date("2026-10-07", &tz), Some(1_791_331_200_000));
        assert_eq!(
            parse_date("2026-10-07T15:00:00", &tz),
            Some(1_791_410_400_000)
        );
        assert_eq!(
            parse_date("2026-10-07T15:00:00.123456Z", &tz),
            Some(1_791_385_200_123)
        );
        assert_eq!(
            parse_date("2026-10-07 15:00:00Z", &tz),
            Some(1_791_385_200_000)
        );
        assert_eq!(parse_date("2026", &tz), Some(1_767_225_600_000));
        assert_eq!(parse_date("2026-13-01", &tz), None);
        assert_eq!(parse_date("2026-02-30", &tz), Some(1_772_409_600_000));
        assert_eq!(parse_date("yesterday", &tz), None);
        assert_eq!(
            parse_date("2026-10-02T12:00:00+02:00", &tz),
            Some(1_790_935_200_000)
        );
        // Values from `TZ=America/Vancouver node -e 'Date.parse(s)'`.
        assert_eq!(
            parse_date("2026-10-07T15:00:00+0200", &tz),
            Some(1_791_378_000_000)
        );
        assert_eq!(
            parse_date("2026-10-07t15:00:00z", &tz),
            Some(1_791_385_200_000)
        );
        assert_eq!(parse_date("+002026-10-07", &tz), Some(1_791_331_200_000));
        assert_eq!(parse_date("-000000-01-01", &tz), None);
        assert_eq!(
            parse_date("2026-10-07T24:00:00Z", &tz),
            Some(1_791_417_600_000)
        );
        assert_eq!(parse_date("2026-10", &tz), Some(1_790_812_800_000));
        assert_eq!(parse_date("2026-10-07T15Z", &tz), None);
        assert_eq!(parse_date("2026-10-07T15:00:60Z", &tz), None);
        assert_eq!(parse_date("2026-10-07T15:00:00 Z", &tz), None);
        assert_eq!(parse_date("2026-10-07T15:00:00+02", &tz), None);
        assert_eq!(parse_date("2026-10-32", &tz), None);
    }

    #[test]
    fn math_round_ties_up() {
        assert_eq!(math_round(2.5), 3.0);
        assert_eq!(math_round(-2.5), -2.0);
        assert_eq!(math_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(math_round(1262.0001), 1262.0);
    }

    #[test]
    fn numbers_serialize_like_json_stringify() {
        assert_eq!(number(12.0).to_string(), "12");
        assert_eq!(number(12.62).to_string(), "12.62");
        let nested = normalize_numbers(serde_json::json!({"a": [1.0, 2.5]}));
        assert_eq!(nested.to_string(), r#"{"a":[1,2.5]}"#);
    }
}
