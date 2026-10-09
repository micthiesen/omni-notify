//! `Date.parse` as V8 implements it (`src/date/dateparser-inl.h`): the ES5 ISO
//! 8601 grammar with V8's extensions, handing whatever it cannot consume to the
//! legacy grammar that accepts RFC 2822 and the many loose forms feeds and
//! services produce. Both read one token stream and share the day, time and
//! zone composers, exactly as V8 does.

use jiff::Span;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;

use crate::js::is_js_whitespace;

/// JS `Date` range limit (`TimeClip`): |ms| above this is an invalid date.
pub const MAX_DATE_MS: i64 = 8_640_000_000_000_000;

/// `Date.parse(input)` in epoch milliseconds; `None` stands for `NaN`.
///
/// ISO date-only forms are UTC and ISO date-times without an offset are local
/// time in `tz`. The legacy grammar accepts month and weekday names,
/// `GMT+0000`, `(comments)`, US zone abbreviations, AM/PM and two-digit years;
/// its zone-less results are local time in `tz`.
pub fn date_parse(input: &str, tz: &TimeZone) -> Option<i64> {
    let mut scanner = Scanner {
        tokens: tokenize(input),
        index: 0,
    };
    let mut day = DayComposer::default();
    let mut time = TimeComposer::default();
    let mut zone = ZoneComposer::default();
    let first = parse_es5(&mut scanner, &mut day, &mut time, &mut zone);
    if first == Token::Invalid {
        return None;
    }
    let mut has_read_number = day.index > 0;
    let mut token = first;
    while token != Token::End {
        match token {
            Token::Number { value: n, .. } => {
                has_read_number = true;
                if scanner.skip_symbol(':') {
                    if scanner.skip_symbol(':') {
                        if !time.is_empty() {
                            return None;
                        }
                        time.add(n);
                        time.add(0);
                    } else {
                        if !time.add(n) {
                            return None;
                        }
                        if scanner.peek().is_symbol('.') {
                            scanner.next();
                        }
                    }
                } else if scanner.skip_symbol('.') && time.is_expecting(n) {
                    time.add(n);
                    let next = scanner.peek();
                    if !next.is_number() {
                        return None;
                    }
                    scanner.next();
                    time.add_final(read_milliseconds(next));
                } else if zone.is_expecting(n) {
                    zone.minute = Some(n);
                } else if time.is_expecting(n) {
                    time.add_final(n);
                    // End, white space, "Z", "+" or "-" must follow a final time value.
                    let peek = scanner.peek();
                    let ends = matches!(peek, Token::End | Token::WhiteSpace)
                        || peek.is_keyword_z()
                        || peek.is_sign();
                    if !ends {
                        return None;
                    }
                } else {
                    if !day.add(n) {
                        return None;
                    }
                    scanner.skip_symbol('-');
                }
            }
            Token::Word { keyword, .. } => match keyword {
                Keyword::AmPm(offset) if !time.is_empty() => time.hour_offset = Some(offset),
                Keyword::Month(month) => {
                    day.named_month = Some(month);
                    scanner.skip_symbol('-');
                }
                Keyword::Zone(offset) if has_read_number => zone.set(offset),
                _ => {
                    // Garbage words are illegal once a number has been read, and
                    // must be separated from the first number.
                    if has_read_number || scanner.peek().is_number() {
                        return None;
                    }
                }
            },
            t if t.is_sign() && (zone.is_utc() || !time.is_empty()) => {
                // A UTC offset (only after UTC or a time).
                zone.sign = Some(if t == Token::Symbol('-') { -1 } else { 1 });
                let (n, length) = match scanner.peek() {
                    Token::Number { value, length } => {
                        scanner.next();
                        (value, length)
                    }
                    _ => (0, 0),
                };
                has_read_number = true;
                if scanner.peek().is_symbol(':') {
                    zone.hour = Some(n);
                    zone.minute = None;
                } else if length == 1 || length == 2 {
                    zone.hour = Some(n);
                    zone.minute = Some(0);
                } else if length == 3 || length == 4 {
                    zone.hour = Some(n / 100);
                    zone.minute = Some(n % 100);
                } else {
                    return None;
                }
            }
            t if (t.is_sign() || t.is_symbol(')')) && has_read_number => return None,
            _ => {}
        }
        token = scanner.next();
    }
    let (year, month, day_of_month) = day.write()?;
    let time_ms = time.write()?;
    let midnight = make_day(year, month, day_of_month)?.to_datetime(Time::midnight());
    let ms = match zone.write() {
        Some(offset_ms) => {
            let utc_midnight = midnight.to_zoned(TimeZone::UTC).ok()?;
            utc_midnight.timestamp().as_millisecond() + time_ms - offset_ms
        }
        None => {
            let local = midnight
                .checked_add(Span::new().milliseconds(time_ms))
                .ok()?;
            local_ms(local, tz)?
        }
    };
    (ms.abs() <= MAX_DATE_MS).then_some(ms)
}

/// `[('-'|'+')yy]yyyy[-MM[-DD]][THH:mm[:ss[.sss]][Z|(+|-)hh[:]mm]]`.
/// Returns the first token it did not handle (`End` after a complete ES5 string),
/// or `Invalid` once a `T` committed it to ISO and the rest does not fit.
fn parse_es5(
    scanner: &mut Scanner,
    day: &mut DayComposer,
    time: &mut TimeComposer,
    zone: &mut ZoneComposer,
) -> Token {
    if scanner.peek().is_sign() {
        // Keep the sign token, so invalid dates can be detected.
        let sign_token = scanner.next();
        if !scanner.peek().is_fixed_length_number(6) {
            return sign_token;
        }
        let year = scanner.next().number();
        if sign_token == Token::Symbol('-') && year == 0 {
            return sign_token;
        }
        day.add(if sign_token == Token::Symbol('-') {
            -year
        } else {
            year
        });
    } else if scanner.peek().is_fixed_length_number(4) {
        day.add(scanner.next().number());
    } else {
        return scanner.next();
    }
    if scanner.skip_symbol('-') {
        let month = scanner.peek();
        if !month.is_fixed_length_number(2) || !(1..=12).contains(&month.number()) {
            return scanner.next();
        }
        day.add(scanner.next().number());
        if scanner.skip_symbol('-') {
            let day_of_month = scanner.peek();
            if !day_of_month.is_fixed_length_number(2) || !(1..=31).contains(&day_of_month.number())
            {
                return scanner.next();
            }
            day.add(scanner.next().number());
        }
    }
    if !scanner.peek().is_time_separator() {
        if scanner.peek() != Token::End {
            return scanner.next();
        }
    } else {
        scanner.next();
        let hour = scanner.peek();
        if !hour.is_fixed_length_number(2) || !(0..=24).contains(&hour.number()) {
            return Token::Invalid;
        }
        // 24:00[:00[.000]] is the end of the day; no other time starts with 24.
        let hour_is_24 = hour.number() == 24;
        time.add(scanner.next().number());
        if !scanner.skip_symbol(':') {
            return Token::Invalid;
        }
        let minute = scanner.peek();
        if !minute.is_fixed_length_number(2)
            || !is_minute(minute.number())
            || (hour_is_24 && minute.number() > 0)
        {
            return Token::Invalid;
        }
        time.add(scanner.next().number());
        if scanner.skip_symbol(':') {
            let second = scanner.peek();
            if !second.is_fixed_length_number(2)
                || !is_minute(second.number())
                || (hour_is_24 && second.number() > 0)
            {
                return Token::Invalid;
            }
            time.add(scanner.next().number());
            if scanner.skip_symbol('.') {
                let fraction = scanner.peek();
                if !fraction.is_number() || (hour_is_24 && fraction.number() > 0) {
                    return Token::Invalid;
                }
                // More or fewer than the mandated three digits are allowed.
                time.add(read_milliseconds(scanner.next()));
            }
        }
        if scanner.peek().is_keyword_z() {
            scanner.next();
            zone.set(0);
        } else if scanner.peek().is_sign() {
            zone.sign = Some(if scanner.next() == Token::Symbol('-') {
                -1
            } else {
                1
            });
            if scanner.peek().is_fixed_length_number(4) {
                // hhmm extension syntax.
                let hourmin = scanner.next().number();
                let (hour, minute) = (hourmin / 100, hourmin % 100);
                if !is_hour(hour) || !is_minute(minute) {
                    return Token::Invalid;
                }
                zone.hour = Some(hour);
                zone.minute = Some(minute);
            } else {
                let hour = scanner.peek();
                if !hour.is_fixed_length_number(2) || !is_hour(hour.number()) {
                    return Token::Invalid;
                }
                zone.hour = Some(scanner.next().number());
                if !scanner.skip_symbol(':') {
                    return Token::Invalid;
                }
                let minute = scanner.peek();
                if !minute.is_fixed_length_number(2) || !is_minute(minute.number()) {
                    return Token::Invalid;
                }
                zone.minute = Some(scanner.next().number());
            }
        }
        if scanner.peek() != Token::End {
            return Token::Invalid;
        }
    }
    // Date-only forms are UTC; date-time forms without an offset are local.
    if zone.hour.is_none() && time.is_empty() {
        zone.set(0);
    }
    day.is_iso_date = true;
    Token::End
}

/// `civil` read as local time in `tz` (a skipped wall time moves forward, an
/// ambiguous one takes the earlier instant, as V8 does).
fn local_ms(civil: DateTime, tz: &TimeZone) -> Option<i64> {
    tz.to_ambiguous_timestamp(civil)
        .compatible()
        .ok()
        .map(|ts| ts.as_millisecond())
}

/// `MakeDay(year, month, 1) + day - 1`: days past the month end roll over.
fn make_day(year: i64, month: i64, day: i64) -> Option<Date> {
    let first = Date::new(i16::try_from(year).ok()?, i8::try_from(month).ok()?, 1).ok()?;
    first.checked_add(Span::new().days(day - 1)).ok()
}

/// `ReadUnsignedNumeral` keeps at most nine significant digits.
const MAX_SIGNIFICANT_DIGITS: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keyword {
    Month(i64),
    AmPm(i64),
    Zone(i64),
    TimeSeparator,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Number { value: i64, length: usize },
    Word { keyword: Keyword, length: usize },
    Symbol(char),
    WhiteSpace,
    Unknown,
    End,
    Invalid,
}

impl Token {
    fn is_number(self) -> bool {
        matches!(self, Token::Number { .. })
    }
    fn is_fixed_length_number(self, n: usize) -> bool {
        matches!(self, Token::Number { length, .. } if length == n)
    }
    fn number(self) -> i64 {
        match self {
            Token::Number { value, .. } => value,
            _ => 0,
        }
    }
    fn is_symbol(self, c: char) -> bool {
        self == Token::Symbol(c)
    }
    fn is_sign(self) -> bool {
        matches!(self, Token::Symbol('+' | '-'))
    }
    fn is_keyword_z(self) -> bool {
        matches!(
            self,
            Token::Word {
                keyword: Keyword::Zone(0),
                length: 1
            }
        )
    }
    fn is_time_separator(self) -> bool {
        matches!(
            self,
            Token::Word {
                keyword: Keyword::TimeSeparator,
                ..
            }
        )
    }
}

/// `KeywordTable::Lookup`: month names match on their first three letters;
/// every other keyword must match exactly.
fn lookup(word: &str) -> Keyword {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = word.to_ascii_lowercase();
    let prefix: String = lower.chars().take(3).collect();
    if let Some(i) = MONTHS.iter().position(|m| *m == prefix) {
        return Keyword::Month(i64::try_from(i).unwrap_or(0) + 1);
    }
    if lower.encode_utf16().count() > 3 {
        return Keyword::Invalid;
    }
    match lower.as_str() {
        "am" => Keyword::AmPm(0),
        "pm" => Keyword::AmPm(12),
        "ut" | "utc" | "z" | "gmt" => Keyword::Zone(0),
        "cdt" => Keyword::Zone(-5),
        "cst" => Keyword::Zone(-6),
        "edt" => Keyword::Zone(-4),
        "est" => Keyword::Zone(-5),
        "mdt" => Keyword::Zone(-6),
        "mst" => Keyword::Zone(-7),
        "pdt" => Keyword::Zone(-7),
        "pst" => Keyword::Zone(-8),
        "t" => Keyword::TimeSeparator,
        _ => Keyword::Invalid,
    }
}

/// `IsAsciiAlphaOrAbove() && !IsWhiteSpaceChar()`.
fn is_word_char(c: char) -> bool {
    c >= 'A' && !is_js_whitespace(c)
}

/// `DateStringTokenizer::Scan` over the whole input.
fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i] == '0' {
                i += 1;
            }
            let mut value = 0i64;
            let mut significant = 0usize;
            while i < chars.len() && chars[i].is_ascii_digit() {
                if significant < MAX_SIGNIFICANT_DIGITS {
                    value = value * 10 + i64::from(u32::from(chars[i]) - u32::from('0'));
                }
                significant += 1;
                i += 1;
            }
            tokens.push(Token::Number {
                value,
                length: i - start,
            });
        } else if matches!(c, ':' | '-' | '+' | '.' | ')') {
            tokens.push(Token::Symbol(c));
            i += 1;
        } else if is_word_char(c) {
            let start = i;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            tokens.push(Token::Word {
                keyword: lookup(&word),
                length: word.encode_utf16().count(),
            });
        } else if is_js_whitespace(c) {
            while i < chars.len() && is_js_whitespace(chars[i]) {
                i += 1;
            }
            tokens.push(Token::WhiteSpace);
        } else if c == '(' {
            let mut depth = 0usize;
            while i < chars.len() {
                match chars[i] {
                    '(' => depth += 1,
                    ')' => depth = depth.saturating_sub(1),
                    _ => {}
                }
                i += 1;
                if depth == 0 {
                    break;
                }
            }
            tokens.push(Token::Unknown);
        } else {
            tokens.push(Token::Unknown);
            i += 1;
        }
    }
    tokens
}

struct Scanner {
    tokens: Vec<Token>,
    index: usize,
}

impl Scanner {
    fn next(&mut self) -> Token {
        let token = self.peek();
        self.index += 1;
        token
    }
    fn peek(&self) -> Token {
        self.tokens.get(self.index).copied().unwrap_or(Token::End)
    }
    fn skip_symbol(&mut self, c: char) -> bool {
        if self.peek().is_symbol(c) {
            self.index += 1;
            true
        } else {
            false
        }
    }
}

fn is_hour(n: i64) -> bool {
    (0..24).contains(&n)
}

fn is_minute(n: i64) -> bool {
    (0..60).contains(&n)
}

fn is_millisecond(n: i64) -> bool {
    (0..1000).contains(&n)
}

/// The first three significant digits of the numeral.
fn read_milliseconds(token: Token) -> i64 {
    let Token::Number { value, length } = token else {
        return 0;
    };
    match length {
        1 => value * 100,
        2 => value * 10,
        0 | 3 => value,
        _ => {
            let excess = length.min(MAX_SIGNIFICANT_DIGITS) - 3;
            value / 10_i64.pow(u32::try_from(excess).unwrap_or(0))
        }
    }
}

#[derive(Default)]
struct TimeComposer {
    comp: [i64; 4],
    index: usize,
    hour_offset: Option<i64>,
}

impl TimeComposer {
    fn is_empty(&self) -> bool {
        self.index == 0
    }
    fn is_expecting(&self, n: i64) -> bool {
        (self.index == 1 && is_minute(n))
            || (self.index == 2 && is_minute(n))
            || (self.index == 3 && is_millisecond(n))
    }
    fn add(&mut self, n: i64) -> bool {
        if self.index < self.comp.len() {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn add_final(&mut self, n: i64) -> bool {
        if !self.add(n) {
            return false;
        }
        self.index = self.comp.len();
        true
    }
    /// Milliseconds since midnight.
    fn write(&self) -> Option<i64> {
        let [mut hour, minute, second, millisecond] = self.comp;
        if let Some(offset) = self.hour_offset {
            if !(0..=12).contains(&hour) {
                return None;
            }
            hour = hour % 12 + offset;
        }
        let valid =
            is_hour(hour) && is_minute(minute) && is_minute(second) && is_millisecond(millisecond);
        if !valid && !(hour == 24 && minute == 0 && second == 0 && millisecond == 0) {
            return None;
        }
        Some(((hour * 60 + minute) * 60 + second) * 1000 + millisecond)
    }
}

#[derive(Default)]
struct ZoneComposer {
    sign: Option<i64>,
    hour: Option<i64>,
    minute: Option<i64>,
}

impl ZoneComposer {
    fn set(&mut self, offset_hours: i64) {
        self.sign = Some(if offset_hours < 0 { -1 } else { 1 });
        self.hour = Some(offset_hours.abs());
        self.minute = Some(0);
    }
    fn is_expecting(&self, n: i64) -> bool {
        self.hour.is_some() && self.minute.is_none() && is_minute(n)
    }
    fn is_utc(&self) -> bool {
        self.hour == Some(0) && self.minute == Some(0)
    }
    /// Offset in milliseconds, or `None` for local time.
    fn write(&self) -> Option<i64> {
        let sign = self.sign?;
        let hour = self.hour.unwrap_or(0);
        let minute = self.minute.unwrap_or(0);
        Some(sign * (hour * 3600 + minute * 60) * 1000)
    }
}

#[derive(Default)]
struct DayComposer {
    comp: [i64; 3],
    index: usize,
    named_month: Option<i64>,
    is_iso_date: bool,
}

impl DayComposer {
    fn add(&mut self, n: i64) -> bool {
        if self.index < self.comp.len() {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }
    /// `(year, month 1-12, day)`.
    fn write(&mut self) -> Option<(i64, i64, i64)> {
        if self.index < 1 {
            return None;
        }
        // Day and month default to 1.
        while self.index < self.comp.len() {
            self.comp[self.index] = 1;
            self.index += 1;
        }
        let is_day = |n: i64| (1..=31).contains(&n);
        let (mut year, month, day) = match self.named_month {
            None if self.is_iso_date || !is_day(self.comp[0]) => {
                (self.comp[0], self.comp[1], self.comp[2])
            }
            None => (self.comp[2], self.comp[0], self.comp[1]),
            Some(month) if !is_day(self.comp[0]) => (self.comp[0], month, self.comp[1]),
            Some(month) => (self.comp[1], month, self.comp[0]),
        };
        if !self.is_iso_date {
            if (0..=49).contains(&year) {
                year += 2000;
            } else if (50..=99).contains(&year) {
                year += 1900;
            }
        }
        ((1..=12).contains(&month) && is_day(day)).then_some((year, month, day))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vancouver() -> TimeZone {
        TimeZone::get("America/Vancouver").unwrap_or(TimeZone::UTC)
    }

    /// Values from `TZ=America/Vancouver node -e 'Date.parse(s)'`.
    #[test]
    fn matches_v8() {
        let tz = vancouver();
        let cases: &[(&str, Option<i64>)] = &[
            ("2026-10-07", Some(1_791_331_200_000)),
            ("2026-10-07T15:00:00", Some(1_791_410_400_000)),
            ("2026-10-07T15:00:00.123456Z", Some(1_791_385_200_123)),
            ("2026-10-07 15:00:00Z", Some(1_791_385_200_000)),
            ("2026", Some(1_767_225_600_000)),
            ("2026-13-01", None),
            ("2026-02-30", Some(1_772_409_600_000)),
            ("yesterday", None),
            ("2026-10-02T12:00:00+02:00", Some(1_790_935_200_000)),
            ("2026-10-07T15:00:00+0200", Some(1_791_378_000_000)),
            ("2026-10-07t15:00:00z", Some(1_791_385_200_000)),
            ("+002026-10-07", Some(1_791_331_200_000)),
            ("-000000-01-01", Some(978_336_000_000)),
            ("2026-10-07T24:00:00Z", Some(1_791_417_600_000)),
            ("2026-10", Some(1_790_812_800_000)),
            ("2026-10-07T15Z", None),
            ("2026-10-07T15:00:00 Z", None),
            ("2026-10-07T15:00:00+02", None),
            ("2026-10-07T15:00:00+2:00", None),
            ("2026-10-07T15:00:00,123Z", None),
            (" 2026-10-07T15:00:00Z", None),
            (" 2025-01-02", Some(1_735_804_800_000)),
            ("2026-10-32", None),
            ("0049-01-01 10:00", Some(2_493_133_200_000)),
            ("2025-01-02T10:00:00.0000000001Z", Some(1_735_812_000_000)),
            ("Thu, 02 Jan 2025 12:00:00 GMT", Some(1_735_819_200_000)),
            ("Thu, 02 Jan 2025 07:00:00 EST", Some(1_735_819_200_000)),
            ("Tue,01 Jul 2025 10:00:00 GMT", Some(1_751_364_000_000)),
            ("3/14/2025 2:30 PM", Some(1_741_987_800_000)),
            ("March 14", Some(984_556_800_000)),
            ("Tue, 01 Jul 2025 10:00:00 CEST", None),
            (
                "Tue, 01 Jul 2025 10:00:00 +0000 (UTC)",
                Some(1_751_364_000_000),
            ),
            ("Mon, 09 Mar 2025 02:30:00", Some(1_741_516_200_000)),
            ("Mon, 03 Mar 2025 12:00:00 GMT+12345", None),
            ("Mon, 03 Mar 2025 12:00:00 GMT+", None),
            ("Mar 3 2025 12:00:00 GMT+000000123", None),
            ("2025_01 Jul 2025", None),
            ("Jul 1 2025 [x]", None),
            ("1741003200000", None),
            ("", None),
        ];
        for (input, expected) in cases {
            assert_eq!(date_parse(input, &tz), *expected, "{input}");
        }
    }
}
