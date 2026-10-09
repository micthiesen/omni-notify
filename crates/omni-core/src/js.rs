//! JS-exact semantics for values that were persisted or hashed by the former
//! TypeScript service. Everything that must match V8 byte for byte goes through
//! here.
//!
//! `tests/js_golden.rs` checks every function against committed V8 output
//! (`tests/golden/js.json`).

use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::LazyLock;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

/// `str.length` in JS: the number of UTF-16 code units.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `str.slice(start, end)` for non-negative indices, in UTF-16 code units.
///
/// Indices clamp to the string length and `start >= end` yields `""`, like JS.
/// When an index splits a surrogate pair, JS produces a lone surrogate, which
/// node writes to UTF-8 (and therefore to SQLite/CBOR) as U+FFFD. This function
/// returns that persisted form: each split half becomes U+FFFD.
pub fn utf16_slice(s: &str, start: usize, end: usize) -> Cow<'_, str> {
    let len = utf16_len(s);
    let start = start.min(len);
    let end = end.min(len);
    if start >= end {
        return Cow::Borrowed("");
    }

    let mut offset = 0usize;
    let mut first_byte = None;
    let mut last_byte = 0usize;
    let mut lead_split = false;
    let mut trail_split = false;
    for (index, ch) in s.char_indices() {
        let width = ch.len_utf16();
        let unit_end = offset + width;
        if offset >= end {
            break;
        }
        if unit_end > start {
            let split_at_start = width == 2 && start == offset + 1;
            let split_at_end = width == 2 && end == offset + 1;
            if split_at_start {
                lead_split = true;
            } else if split_at_end {
                trail_split = true;
            } else {
                first_byte.get_or_insert(index);
                last_byte = index + ch.len_utf8();
            }
        }
        offset = unit_end;
    }
    let middle = match first_byte {
        Some(first) => &s[first..last_byte],
        None => "",
    };
    if !lead_split && !trail_split {
        return Cow::Borrowed(middle);
    }
    let mut out = String::with_capacity(middle.len() + 6);
    if lead_split {
        out.push('\u{FFFD}');
    }
    out.push_str(middle);
    if trail_split {
        out.push('\u{FFFD}');
    }
    Cow::Owned(out)
}

/// `Number.prototype.toString()` (radix 10).
pub fn number_to_string(n: f64) -> String {
    let mut buffer = ryu_js::Buffer::new();
    buffer.format(n).to_owned()
}

/// `Number(s)` for a string (ECMA-262 `StringToNumber`): surrounding JS
/// whitespace is ignored, `""` is 0, `0x`/`0o`/`0b` literals are accepted
/// without a sign, `Infinity` with an optional sign, decimal literals without
/// numeric separators; anything else is `NaN`.
pub fn string_to_number(s: &str) -> f64 {
    let trimmed = s.trim_matches(is_js_whitespace);
    if trimmed.is_empty() {
        return 0.0;
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => Some(16),
            b'o' | b'O' => Some(8),
            b'b' | b'B' => Some(2),
            _ => None,
        };
        if let Some(radix) = radix {
            return parse_radix_integer(&trimmed[2..], radix);
        }
    }
    let unsigned = trimmed
        .strip_prefix('+')
        .or_else(|| trimmed.strip_prefix('-'))
        .unwrap_or(trimmed);
    let negative = trimmed.starts_with('-');
    if unsigned == "Infinity" {
        return if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    if !is_decimal_literal(unsigned) {
        return f64::NAN;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

/// `StrWhiteSpaceChar` (and regex `\s`): WhiteSpace (including every `Zs`
/// character and U+FEFF) and LineTerminator. Unlike `char::is_whitespace`, U+0085
/// is not whitespace.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `String.prototype.trimStart`.
pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

/// `String.prototype.trimEnd`.
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

/// `Date.parse(s)` in epoch ms with zone-less forms read in `tz`; `None` is `NaN`.
pub use crate::js_date::{MAX_DATE_MS, date_parse};

/// `Number.prototype.toFixed(digits)`: the decimal with `digits` fraction digits
/// nearest the exact binary value, exact ties rounded away from zero (Rust's
/// formatter rounds ties to even). `|x| >= 1e21` and non-finite values print as
/// `Number#toString`, and `-0` prints without a sign.
pub fn to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() || value.abs() >= 1e21 {
        return number_to_string(value);
    }
    let value = if value == 0.0 { 0.0 } else { value };
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

/// `Number.MAX_SAFE_INTEGER` as a double.
const MAX_SAFE_INTEGER_F64: f64 = 9_007_199_254_740_991.0;

/// A JS number as a JSON value the way `JSON.stringify` writes it: integral
/// values within the safe range become JSON integers (`12`, not `12.0`, and
/// `-0` becomes `0`); non-finite numbers become `null`.
pub fn number_value(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() <= MAX_SAFE_INTEGER_F64 {
        // Exact: integral and within +-2^53.
        #[allow(clippy::cast_possible_truncation)]
        let int = n as i64;
        Value::from(int)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

/// Rewrites every integral float in `value` as a JSON integer ([`number_value`]):
/// JS has a single number type, so `2.0` and `2` are the same value.
pub fn normalize_numbers(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => n.as_f64().map_or(Value::Number(n), number_value),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, normalize_numbers(v)))
                .collect(),
        ),
        other => other,
    }
}

/// `StrUnsignedDecimalLiteral` without the `Infinity` case.
fn is_decimal_literal(s: &str) -> bool {
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(index) => (&s[..index], Some(&s[index + 1..])),
        None => (s, None),
    };
    let (int_part, frac_part) = match mantissa.find('.') {
        Some(index) => (&mantissa[..index], Some(&mantissa[index + 1..])),
        None => (mantissa, None),
    };
    let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    let has_digits = !int_part.is_empty() || frac_part.is_some_and(|f| !f.is_empty());
    if !has_digits || !all_digits(int_part) || !frac_part.is_none_or(all_digits) {
        return false;
    }
    match exponent {
        None => true,
        Some(exp) => {
            let digits = exp
                .strip_prefix('+')
                .or_else(|| exp.strip_prefix('-'))
                .unwrap_or(exp);
            !digits.is_empty() && all_digits(digits)
        }
    }
}

/// A non-decimal integer literal's value, rounded to the nearest double.
fn parse_radix_integer(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut exact: u128 = 0;
    let mut approx = 0.0f64;
    let mut overflowed = false;
    for ch in digits.chars() {
        let Some(digit) = ch.to_digit(radix) else {
            return f64::NAN;
        };
        if !overflowed {
            match exact
                .checked_mul(u128::from(radix))
                .and_then(|v| v.checked_add(u128::from(digit)))
            {
                Some(next) => exact = next,
                None => {
                    overflowed = true;
                    #[allow(clippy::cast_precision_loss)]
                    {
                        approx = exact as f64;
                    }
                }
            }
        }
        if overflowed {
            approx = approx * f64::from(radix) + f64::from(digit);
        }
    }
    if overflowed {
        approx
    } else {
        #[allow(clippy::cast_precision_loss)]
        let value = exact as f64;
        value
    }
}

/// `JSON.stringify(v)`.
///
/// Object keys follow JS property order: canonical array-index keys first in
/// ascending numeric order, then the remaining keys in insertion order.
/// Numbers are formatted as JS doubles.
pub fn json_stringify(v: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, v, None, 0);
    out
}

/// `JSON.stringify(v, null, 2)`.
pub fn json_stringify_pretty2(v: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, v, Some(2), 0);
    out
}

fn write_value(out: &mut String, v: &Value, indent: Option<usize>, depth: usize) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() => out.push_str(&number_to_string(f)),
            _ => out.push_str("null"),
        },
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, depth + 1);
                write_value(out, item, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, value)) in js_property_order(map).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, depth + 1);
                write_string(out, key);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, value, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push('}');
        }
    }
}

fn newline(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(width) = indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', width * depth));
    }
}

/// Canonical array index per ECMA-262: "0" or no leading zero, below 2^32 - 1.
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u64>()
        .ok()
        .filter(|n| *n < u64::from(u32::MAX))
        .and_then(|n| u32::try_from(n).ok())
}

fn js_property_order(map: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut indexed: Vec<(u32, &String, &Value)> = Vec::new();
    let mut named: Vec<(&String, &Value)> = Vec::new();
    for (key, value) in map {
        match array_index(key) {
            Some(index) => indexed.push((index, key, value)),
            None => named.push((key, value)),
        }
    }
    indexed.sort_by_key(|(index, _, _)| *index);
    indexed
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .chain(named)
        .collect()
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

static ROOT_COLLATOR: LazyLock<Option<icu_collator::CollatorBorrowed<'static>>> =
    LazyLock::new(|| {
        icu_collator::Collator::try_new(
            icu_collator::CollatorPreferences::default(),
            icu_collator::options::CollatorOptions::default(),
        )
        .ok()
    });

/// `a.localeCompare(b)` with V8's default (root, tertiary) ICU collation.
///
/// The compiled root collation data cannot fail to load; if it ever did, the
/// comparison falls back to UTF-16 code-unit order (JS `<`), which is the
/// closest locale-independent ordering.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    match ROOT_COLLATOR.as_ref() {
        Some(collator) => collator.compare(a, b),
        None => a.encode_utf16().cmp(b.encode_utf16()),
    }
}

/// Characters `encodeURIComponent` leaves unescaped: `A-Z a-z 0-9 - _ . ! ~ * ' ( )`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// `encodeURIComponent(s)`.
pub fn encode_uri_component(s: &str) -> String {
    utf8_percent_encode(s, URI_COMPONENT).to_string()
}

/// `new Date(ms).toISOString()`: always `.mmmZ`, six-digit signed years outside 0..=9999.
///
/// JS throws a `RangeError` beyond ±8.64e15 ms; this function formats any `i64`
/// with the same extended-year rule instead.
pub fn to_iso_string(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let ms_of_day = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = ms_of_day / 3_600_000;
    let minute = (ms_of_day / 60_000) % 60;
    let second = (ms_of_day / 1_000) % 60;
    let millis = ms_of_day % 1_000;
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", -year)
    } else {
        format!("+{year:06}")
    };
    format!("{year}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Proleptic Gregorian date for days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    // d is in 1..=31 and m in 1..=12 by construction.
    (
        year,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_lengths() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("é"), 1);
        assert_eq!(utf16_len("😀"), 2);
    }

    #[test]
    fn utf16_slices() {
        assert_eq!(utf16_slice("hello", 1, 3), "el");
        assert_eq!(utf16_slice("hello", 3, 1), "");
        assert_eq!(utf16_slice("hello", 2, 99), "llo");
        assert_eq!(utf16_slice("a😀b", 0, 3), "a😀");
        assert_eq!(utf16_slice("a😀b", 0, 2), "a\u{FFFD}");
        assert_eq!(utf16_slice("a😀b", 2, 4), "\u{FFFD}b");
        assert_eq!(utf16_slice("😀", 1, 2), "\u{FFFD}");
    }

    #[test]
    fn numbers_format_like_v8() {
        assert_eq!(number_to_string(1.0), "1");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(0.1), "0.1");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
    }

    #[test]
    fn stringify_matches_js() {
        let v = json!({"b": 1, "a": [true, null, "x\n\u{1}"], "2": 2.5, "10": {}, "01": []});
        assert_eq!(
            json_stringify(&v),
            r#"{"2":2.5,"10":{},"b":1,"a":[true,null,"x\n\u0001"],"01":[]}"#
        );
        assert_eq!(
            json_stringify_pretty2(&json!({"a": [1, {"b": []}]})),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": []\n    }\n  ]\n}"
        );
    }

    #[test]
    fn trims_js_whitespace_only() {
        assert_eq!(trim("\u{FEFF} a\u{A0}\u{3000}"), "a");
        assert_eq!(trim("\u{85}a\u{85}"), "\u{85}a\u{85}");
        assert_eq!(trim_start("\u{2028} a "), "a ");
        assert_eq!(trim_end(" a \u{2029}"), " a");
    }

    /// Values from node's `Number#toFixed`.
    #[test]
    fn to_fixed_matches_js() {
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(0.0625, 3), "0.063");
        assert_eq!(to_fixed(0.5, 0), "1");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(0.92, 3), "0.920");
        assert_eq!(to_fixed(1.0, 3), "1.000");
        assert_eq!(to_fixed(30.000_000_000_000_004, 0), "30");
        assert_eq!(to_fixed(-0.125, 2), "-0.13");
        assert_eq!(to_fixed(-0.0, 2), "0.00");
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(99.5, 0), "100");
        assert_eq!(to_fixed(1e21, 2), "1e+21");
        assert_eq!(to_fixed(f64::NAN, 2), "NaN");
    }

    #[test]
    fn numbers_normalize_like_json_stringify() {
        assert_eq!(number_value(12.0).to_string(), "12");
        assert_eq!(number_value(-0.0).to_string(), "0");
        assert_eq!(number_value(12.5).to_string(), "12.5");
        assert_eq!(number_value(f64::NAN), Value::Null);
        let nested = normalize_numbers(json!({"a": [1.0, 2.5, {"b": 3.0}], "c": 4}));
        assert_eq!(nested.to_string(), r#"{"a":[1,2.5,{"b":3}],"c":4}"#);
    }

    #[test]
    fn locale_compare_is_case_insensitive_first() {
        assert_eq!(locale_compare("a", "B"), Ordering::Less);
        assert_eq!(locale_compare("a", "A"), Ordering::Less);
        assert_eq!(locale_compare("b", "a"), Ordering::Greater);
    }

    #[test]
    fn uri_component_encoding() {
        assert_eq!(
            encode_uri_component("a b/c?d=é!*'()"),
            "a%20b%2Fc%3Fd%3D%C3%A9!*'()"
        );
    }

    #[test]
    fn iso_strings() {
        assert_eq!(to_iso_string(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(to_iso_string(1_760_000_000_123), "2025-10-09T08:53:20.123Z");
        assert_eq!(to_iso_string(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(
            to_iso_string(8_640_000_000_000_000),
            "+275760-09-13T00:00:00.000Z"
        );
        assert_eq!(
            to_iso_string(-62_198_755_200_000),
            "-000001-01-01T00:00:00.000Z"
        );
    }
}
