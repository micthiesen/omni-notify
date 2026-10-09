//! Small pieces shared by the recommendation and episode pages.

use omni_web_kit::utils::js::{js_round, number_string};

/// Minutes as `1h 5m`, `2h`, `45m`.
pub fn format_minutes(minutes: f64) -> String {
    let h = (minutes / 60.0).floor();
    let m = js_round(minutes % 60.0);
    if h > 0.0 {
        if m > 0.0 {
            format!("{}h {}m", number_string(h), number_string(m))
        } else {
            format!("{}h", number_string(h))
        }
    } else {
        format!("{}m", number_string(m))
    }
}

#[cfg(test)]
mod tests {
    use super::format_minutes;

    #[test]
    fn minutes_format_matches_pinned_cases() {
        assert_eq!(format_minutes(45.0), "45m");
        assert_eq!(format_minutes(60.0), "1h");
        assert_eq!(format_minutes(125.0), "2h 5m");
        assert_eq!(format_minutes(0.0), "0m");
        // `Math.round(59.6 % 60)` is 60, which is printed as is.
        assert_eq!(format_minutes(59.6), "60m");
    }
}
