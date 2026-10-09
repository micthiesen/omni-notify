//! JS-compatible helpers this package needs beyond `omni_core::js`: `Date.parse`
//! for feed and Castro dates, `Number#toFixed`, and date stamps.

use jiff::Timestamp;
use jiff::civil::DateTime;
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

/// `Date.parse` for the shapes feeds and Castro produce. Strict ISO 8601 /
/// RFC 3339 first (no offset means local time, a bare date means UTC), then a
/// port of V8's legacy date parser (`dateparser-inl.h`), which is what
/// `Date.parse` falls back to for RFC 2822 and the many loose forms feeds use
/// (`July`, `Tuesday,`, `GMT+0000`, `(UTC)` comments, US zone names, AM/PM).
/// Zone-less results are read in `tz`. `None` stands for `NaN`.
pub fn parse_date(raw: &str, tz: &TimeZone) -> Option<i64> {
    let input = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if input.is_empty() {
        return None;
    }
    if let Ok(ts) = input.parse::<Timestamp>() {
        return Some(ts.as_millisecond());
    }
    if let Ok(date) = input.parse::<jiff::civil::Date>()
        && input.len() == 10
    {
        return date
            .to_zoned(TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp().as_millisecond());
    }
    if input.contains('T')
        && let Ok(dt) = input.parse::<DateTime>()
    {
        return local_ms(dt, tz);
    }
    legacy::parse(&input, tz)
}

fn local_ms(dt: DateTime, tz: &TimeZone) -> Option<i64> {
    dt.to_zoned(tz.clone())
        .ok()
        .map(|z| z.timestamp().as_millisecond())
}

/// V8's legacy `Date.parse` grammar: a token loop feeding day, time and zone
/// composers, garbage words tolerated only before the first number.
mod legacy {
    use jiff::civil::Date;
    use jiff::tz::TimeZone;

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
    }

    impl Token {
        fn is_number(self) -> bool {
            matches!(self, Token::Number { .. })
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
    }

    /// `KeywordTable::Lookup`: month names match on their first three letters;
    /// every other keyword must match exactly.
    fn lookup(word: &str) -> Keyword {
        let lower = word.to_lowercase();
        let prefix: String = lower.chars().take(3).collect();
        let exact = lower.chars().count() <= 3;
        const MONTHS: [&str; 12] = [
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        ];
        if let Some(i) = MONTHS.iter().position(|m| *m == prefix) {
            return Keyword::Month(i64::try_from(i).unwrap_or(0) + 1);
        }
        if !exact {
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

    fn is_word_char(c: char) -> bool {
        c.is_ascii_alphabetic() || (!c.is_ascii() && !c.is_whitespace())
    }

    fn tokenize(input: &str) -> Vec<Token> {
        let chars: Vec<char> = input.chars().collect();
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_digit() {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let digits: String = chars[start..i]
                    .iter()
                    .take(MAX_SIGNIFICANT_DIGITS)
                    .collect();
                tokens.push(Token::Number {
                    value: digits.parse().unwrap_or(0),
                    length: i - start,
                });
            } else if is_word_char(c) {
                let start = i;
                while i < chars.len() && is_word_char(chars[i]) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                tokens.push(Token::Word {
                    keyword: lookup(&word),
                    length: i - start,
                });
            } else if c.is_whitespace() {
                while i < chars.len() && chars[i].is_whitespace() {
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
                tokens.push(Token::Symbol(c));
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

    fn is_minute(n: i64) -> bool {
        (0..60).contains(&n)
    }

    fn is_millisecond(n: i64) -> bool {
        (0..1000).contains(&n)
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
            let valid = (0..24).contains(&hour)
                && is_minute(minute)
                && is_minute(second)
                && is_millisecond(millisecond);
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
            while self.index < self.comp.len() {
                self.comp[self.index] = 1;
                self.index += 1;
            }
            let is_day = |n: i64| (1..=31).contains(&n);
            let (mut year, month, day) = match self.named_month {
                None if !is_day(self.comp[0]) => (self.comp[0], self.comp[1], self.comp[2]),
                None => (self.comp[2], self.comp[0], self.comp[1]),
                Some(month) if !is_day(self.comp[0]) => (self.comp[0], month, self.comp[1]),
                Some(month) => (self.comp[1], month, self.comp[0]),
            };
            if (0..=49).contains(&year) {
                year += 2000;
            } else if (50..=99).contains(&year) {
                year += 1900;
            }
            ((1..=12).contains(&month) && is_day(day)).then_some((year, month, day))
        }
    }

    pub(super) fn parse(input: &str, tz: &TimeZone) -> Option<i64> {
        let mut scanner = Scanner {
            tokens: tokenize(input),
            index: 0,
        };
        let mut day = DayComposer::default();
        let mut time = TimeComposer::default();
        let mut zone = ZoneComposer::default();
        let mut has_read_number = false;
        loop {
            let token = scanner.next();
            match token {
                Token::End => break,
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
                        let Token::Number { value, length } = scanner.peek() else {
                            return None;
                        };
                        scanner.next();
                        // `ReadMilliseconds`: the first three digits, scaled.
                        let ms = match length {
                            1 => value * 100,
                            2 => value * 10,
                            3 => value,
                            _ => {
                                let digits =
                                    i64::try_from(length.min(MAX_SIGNIFICANT_DIGITS)).unwrap_or(9);
                                value / 10_i64.pow(u32::try_from(digits - 3).unwrap_or(0))
                            }
                        };
                        if !time.add_final(ms) {
                            return None;
                        }
                    } else if zone.is_expecting(n) {
                        zone.minute = Some(n);
                    } else if time.is_expecting(n) {
                        time.add_final(n);
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
                        // Garbage words are illegal once a number has been read,
                        // and must be separated from the first number.
                        if has_read_number || scanner.peek().is_number() {
                            return None;
                        }
                    }
                },
                t if t.is_sign() && (zone.is_utc() || !time.is_empty()) => {
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
                    } else {
                        zone.hour = Some(n / 100);
                        zone.minute = Some(n % 100);
                    }
                }
                t if (t.is_sign() || t.is_symbol(')')) && has_read_number => return None,
                _ => {}
            }
        }
        let (year, month, day_of_month) = day.write()?;
        let time_ms = time.write()?;
        // `MakeDay` does not validate the day against the month (Feb 30 is Mar 2).
        let first = Date::new(i16::try_from(year).ok()?, i8::try_from(month).ok()?, 1).ok()?;
        let days = first
            .checked_add(jiff::Span::new().days(day_of_month - 1))
            .ok()?;
        let midnight = days.to_datetime(jiff::civil::Time::midnight());
        match zone.write() {
            Some(offset_ms) => {
                let utc_midnight = midnight.to_zoned(TimeZone::UTC).ok()?;
                Some(utc_midnight.timestamp().as_millisecond() + time_ms - offset_ms)
            }
            None => {
                let local = midnight
                    .checked_add(jiff::Span::new().milliseconds(time_ms))
                    .ok()?;
                super::local_ms(local, tz)
            }
        }
    }
}

/// `Number#toFixed(digits)`: the nearest decimal with `digits` fraction digits,
/// exact ties rounded away from zero (Rust's formatter rounds ties to even).
pub fn to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return omni_core::js::number_to_string(value);
    }
    if value.abs() >= 1e21 {
        return omni_core::js::number_to_string(value);
    }
    let rounded = format!("{value:.digits$}");
    // The exact binary expansion decides whether this was a tie.
    let exact = format!("{:.1100}", value.abs());
    let Some(point) = exact.find('.') else {
        return rounded;
    };
    let tail = &exact[point + 1..];
    let after = tail.get(digits..).unwrap_or("");
    let is_tie = after.starts_with('5') && after[1..].bytes().all(|b| b == b'0');
    if !is_tie {
        return rounded;
    }
    // Round the magnitude up: truncate, then add one unit in the last place.
    let kept = format!("{}{}", &exact[..point], &tail[..digits.min(tail.len())]);
    let incremented = increment_decimal_digits(&kept);
    let (int_part, frac_part) = incremented.split_at(incremented.len() - digits);
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    let sign = if value < 0.0 { "-" } else { "" };
    if digits == 0 {
        format!("{sign}{int_part}")
    } else {
        format!("{sign}{int_part}.{frac_part}")
    }
}

fn increment_decimal_digits(digits: &str) -> String {
    let mut bytes: Vec<u8> = digits.bytes().collect();
    let mut i = bytes.len();
    loop {
        if i == 0 {
            bytes.insert(0, b'1');
            break;
        }
        i -= 1;
        if bytes[i] == b'9' {
            bytes[i] = b'0';
        } else {
            bytes[i] += 1;
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

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
