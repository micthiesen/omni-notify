//! JS `Math.max`/`Math.min`, which propagate `NaN` where `f64::max`/`f64::min`
//! ignore it.

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
    fn max_and_min_propagate_nan() {
        assert!(js_max(1.0, f64::NAN).is_nan());
        assert!(js_min(f64::NAN, 1.0).is_nan());
        assert_eq!(js_max(1.0, 2.0), 2.0);
        assert_eq!(js_min(1.0, 2.0), 1.0);
    }
}
