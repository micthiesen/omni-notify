//! JS string semantics the ported guards depend on.

/// `String.prototype.trim` whitespace: ECMAScript WhiteSpace plus LineTerminator.
///
/// Unlike Rust's `char::is_whitespace`, this includes U+FEFF and excludes U+0085,
/// so trimmed text (and the fingerprints hashed from it) matches TS exactly.
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

/// `value.trim()` with JS whitespace.
pub fn trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_like_javascript() {
        assert_eq!(trim("\u{FEFF} a b \u{3000}\n"), "a b");
        assert_eq!(trim("\u{0085}x\u{0085}"), "\u{0085}x\u{0085}");
        assert_eq!(trim(" \t "), "");
    }
}
