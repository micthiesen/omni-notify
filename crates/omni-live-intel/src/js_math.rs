//! JS `Math` semantics where Rust's differ.

pub use omni_core::js::{is_js_whitespace, to_fixed as js_to_fixed, trim as js_trim};

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
