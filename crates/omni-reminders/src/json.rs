//! Small JS-semantics helpers for untrusted JSON (`typeof`, `Number.isSafeInteger`,
//! `Buffer.from(s, "base64")`).

use serde_json::{Map, Value};

/// `Number.MAX_SAFE_INTEGER`.
pub(crate) const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The number a JSON value holds, as JS sees it.
pub(crate) fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// `Number.isInteger(v)`: a finite number without a fractional part.
pub(crate) fn integer(value: &Value) -> Option<f64> {
    number(value).filter(|n| n.is_finite() && n.fract() == 0.0)
}

/// `Number.isSafeInteger(v)`, as an `i64`.
pub(crate) fn safe_integer(value: &Value) -> Option<i64> {
    integer(value)
        .filter(|n| n.abs() <= MAX_SAFE_INTEGER)
        .map(js_to_i64)
}

/// Converts an integral double inside the safe range to `i64` (normalizing `-0`).
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn js_to_i64(n: f64) -> i64 {
    // Callers only pass integral values within +-2^53, which convert exactly.
    (n + 0.0) as i64
}

/// The `asRecord` helper: the object, or an empty one.
pub(crate) fn as_record(value: &Value) -> &Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    value
        .as_object()
        .unwrap_or_else(|| EMPTY.get_or_init(Map::new))
}

/// A field of an object, or `Null` when absent.
pub(crate) fn field<'a>(object: &'a Map<String, Value>, name: &str) -> &'a Value {
    object.get(name).unwrap_or(&Value::Null)
}

/// `Buffer.from(s, "base64")`: accepts both alphabets, stops at padding,
/// ignores characters outside the alphabet and decodes a partial final group.
pub(crate) fn node_base64(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 2);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in s.bytes() {
        let sextet = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    out
}

/// Standard padded base64 (`Buffer#toString("base64")`).
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// ECMAScript `WhiteSpace` or `LineTerminator` (what `String#trim` removes). Unlike
/// `char::is_whitespace`, this includes U+FEFF and excludes U+0085.
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `String#trim()`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `!s.trim()`: empty after JS trimming.
pub(crate) fn js_blank(s: &str) -> bool {
    js_trim(s).is_empty()
}

/// A JS-truthy (non-empty) optional string.
pub(crate) fn present(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|s| !s.is_empty())
}

/// Bounds a string for error metadata (`slice(0, n)` on UTF-16 units).
pub(crate) fn bounded(s: &str, max: usize) -> String {
    omni_core::js::utf16_slice(s, 0, max).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_like_node_buffer() {
        assert_eq!(node_base64("aGVsbG8="), b"hello");
        assert_eq!(node_base64("aGVsbG8"), b"hello");
        assert_eq!(node_base64("opaque").len(), 4);
        assert_eq!(node_base64("%%%"), Vec::<u8>::new());
    }

    #[test]
    fn trims_like_ecmascript() {
        assert!(js_blank(" \u{feff}\u{3000}\t\n"));
        assert!(!js_blank("\u{85}"));
        assert_eq!(js_trim("\u{a0}x\u{2028}"), "x");
        assert_eq!(present(Some(&String::new())), None);
        assert_eq!(present(Some(&"t".to_owned())), Some("t"));
    }

    #[test]
    fn safe_integers_follow_js() {
        assert_eq!(safe_integer(&json!(5)), Some(5));
        assert_eq!(safe_integer(&json!(5.0)), Some(5));
        assert_eq!(safe_integer(&json!(5.5)), None);
        assert_eq!(safe_integer(&json!(9_007_199_254_740_992_u64)), None);
        assert_eq!(safe_integer(&json!("5")), None);
    }
}
