//! JS string semantics the ported guards depend on.

pub use omni_core::js::{is_js_whitespace, trim};

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
