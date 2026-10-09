//! JS string semantics used by workspace validation and prompts.

use omni_core::js::{utf16_len, utf16_slice};

/// `String#trim`: ECMAScript WhiteSpace and LineTerminator. That is Unicode
/// `White_Space` plus the BOM (which Rust's `trim` keeps) minus U+0085 NEXT LINE
/// (which Rust's `trim` removes and JS keeps).
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

fn is_js_whitespace(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{0085}')
}

/// `value.length` (UTF-16 code units).
pub fn js_len(s: &str) -> usize {
    utf16_len(s)
}

/// `value.slice(0, max)`.
pub fn js_prefix(s: &str, max: usize) -> String {
    utf16_slice(s, 0, max).into_owned()
}

/// The engine's prompt `truncate`: the first `max` units plus `\n[truncated]`.
pub fn truncate_marked(value: &str, max: usize) -> String {
    if js_len(value) <= max {
        value.to_owned()
    } else {
        format!("{}\n[truncated]", js_prefix(value, max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_like_js() {
        assert_eq!(js_trim("\u{FEFF} hi \n"), "hi");
        assert_eq!(js_trim("\u{00A0}x\u{2028}"), "x");
        assert_eq!(js_trim("\u{0085}x\u{0085}"), "\u{0085}x\u{0085}");
    }

    #[test]
    fn truncates_in_utf16_units() {
        assert_eq!(truncate_marked("abc", 3), "abc");
        assert_eq!(truncate_marked("abcd", 3), "abc\n[truncated]");
        assert_eq!(js_len("é😀"), 3);
    }
}
