//! JS semantics this crate needs: `Date.parse`, JS whitespace and `trim` (all
//! from `omni_core::js`), plus `Math.round`.

use jiff::Timestamp;

pub use omni_core::js::{MAX_DATE_MS, date_parse as parse_date, is_js_whitespace, trim, trim_end};

/// `Math.round`: nearest integer, ties toward positive infinity.
pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// Epoch milliseconds for a timestamp.
pub fn timestamp_ms(ts: Timestamp) -> i64 {
    ts.as_millisecond()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_round_ties_up() {
        assert_eq!(math_round(2.5), 3.0);
        assert_eq!(math_round(-2.5), -2.0);
        assert_eq!(math_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(math_round(1262.0001), 1262.0);
    }
}
