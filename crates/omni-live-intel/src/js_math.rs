//! JS `Math` semantics where Rust's differ.

/// JS `WhiteSpace` or `LineTerminator` (what `String#trim` removes): Unicode
/// `White_Space` minus U+0085, plus U+FEFF.
pub fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `String#trim`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `Math.round`: halves round toward positive infinity.
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `Math.max(a, b)` (NaN-propagating).
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `Math.min(a, b)` (NaN-propagating).
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `Number#toFixed(digits)`. JS rounds the exact binary value half away from
/// zero (`0.0625.toFixed(3)` is `"0.063"`), where Rust's formatter rounds an
/// exact tie to even, so the rounding is done here on the exact expansion.
pub fn js_to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return omni_core::js::number_to_string(x);
    }
    let negative = x < 0.0;
    // 1100 fractional digits hold every f64 exactly.
    let exact = format!("{:.1100}", x.abs());
    let (int_part, frac_part) = exact.split_once('.').unwrap_or((exact.as_str(), ""));
    let mut kept: Vec<u8> = int_part
        .bytes()
        .chain(frac_part.bytes().take(digits))
        .collect();
    if frac_part.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        let mut index = kept.len();
        loop {
            if index == 0 {
                kept.insert(0, b'1');
                break;
            }
            index -= 1;
            if kept[index] == b'9' {
                kept[index] = b'0';
            } else {
                kept[index] += 1;
                break;
            }
        }
    }
    let split = kept.len() - digits;
    let (int_digits, frac_digits) = kept.split_at(split);
    let mut out = String::with_capacity(kept.len() + 2);
    if negative {
        out.push('-');
    }
    out.push_str(&String::from_utf8_lossy(int_digits));
    if digits > 0 {
        out.push('.');
        out.push_str(&String::from_utf8_lossy(frac_digits));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_like_math_round() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(js_round(70.67), 71.0);
    }

    #[test]
    fn trims_like_string_trim() {
        assert_eq!(js_trim("\u{feff}\u{a0} note \u{2028}"), "note");
        assert_eq!(js_trim("\u{85}note\u{85}"), "\u{85}note\u{85}");
    }

    #[test]
    fn to_fixed_matches_node() {
        // Expected values from node's Number#toFixed.
        let cases: [(f64, usize, &str); 12] = [
            (0.0625, 3, "0.063"),
            (0.5625, 3, "0.563"),
            (0.1875, 3, "0.188"),
            (0.25, 1, "0.3"),
            (0.0005, 3, "0.001"),
            (1.0005, 3, "1.000"),
            (0.755, 3, "0.755"),
            (0.999_95, 3, "1.000"),
            (9.96, 1, "10.0"),
            (-0.0625, 3, "-0.063"),
            (-0.0001, 3, "-0.000"),
            (18.0, 1, "18.0"),
        ];
        for (x, digits, expected) in cases {
            assert_eq!(js_to_fixed(x, digits), expected, "{x}.toFixed({digits})");
        }
        assert_eq!(js_to_fixed(0.7, 0), "1");
    }
}
