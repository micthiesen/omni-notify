//! JS formatting semantics the recommendation code persists or prompts with,
//! plus serde helpers that mirror Effect Schema / zod optionality.

pub use omni_core::js::{is_js_whitespace, to_fixed, trim as js_trim};
use serde::{Deserialize, Deserializer};

/// `String(n)` for a JS number.
pub fn number(n: f64) -> String {
    omni_core::js::number_to_string(n)
}

/// `Math.round`: halves round toward positive infinity.
pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `toDateStamp`: the UTC `YYYY-MM-DD` of an epoch.
pub fn to_date_stamp(epoch_ms: i64) -> String {
    let iso = omni_core::js::to_iso_string(epoch_ms);
    iso.get(..10).unwrap_or(&iso).to_owned()
}

/// mitools `logTimestamp`: local `YYYY-MM-DDTHH-MM-SS` for log file names.
pub fn log_timestamp(epoch_ms: i64, tz: &jiff::tz::TimeZone) -> String {
    let zoned = omni_core::clock::timestamp_from_ms(epoch_ms).to_zoned(tz.clone());
    zoned.strftime("%Y-%m-%dT%H-%M-%S").to_string()
}

/// mitools `codeBlock`: a fence that does not collide with the content.
pub fn code_block(content: &str, lang: Option<&str>) -> String {
    let mut fence = "```".to_owned();
    while content.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}{}\n{content}\n{fence}", lang.unwrap_or(""))
}

/// `#[serde(default, deserialize_with = "present")]`: an absent field is
/// `None`, but an explicit `null` is rejected (Effect `Schema.optional`, zod
/// `.optional()`).
pub fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Collapses whitespace runs to one space (`replace(/\s+/g, " ")`).
pub fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if is_js_whitespace(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// `s.slice(0, n)` in UTF-16 units.
pub fn slice_utf16(s: &str, n: usize) -> String {
    omni_core::js::utf16_slice(s, 0, n).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_matches_js() {
        assert_eq!(to_fixed(0.0625, 3), "0.063");
        assert_eq!(to_fixed(0.92, 2), "0.92");
        assert_eq!(to_fixed(1.0, 3), "1.000");
        assert_eq!(to_fixed(0.25 * 100.0, 0), "25");
        assert_eq!(to_fixed(0.5, 0), "1");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(9.99, 1), "10.0");
        assert_eq!(to_fixed(-1.5, 0), "-2");
        assert_eq!(to_fixed(8.2, 1), "8.2");
        assert_eq!(to_fixed(75.0, 1), "75.0");
    }

    #[test]
    fn math_round_matches_js() {
        assert!((math_round(2.5) - 3.0).abs() < f64::EPSILON);
        assert!((math_round(-2.5) - -2.0).abs() < f64::EPSILON);
        assert!((math_round(0.494) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn date_stamp_is_utc() {
        assert_eq!(to_date_stamp(1_750_000_000_000), "2025-06-15");
    }

    #[test]
    fn code_block_extends_fence() {
        assert_eq!(code_block("a", None), "```\na\n```");
        assert_eq!(code_block("```", Some("json")), "````json\n```\n````");
    }
}
