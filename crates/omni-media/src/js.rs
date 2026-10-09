//! Text helpers for recommendation prompts, plus serde optionality helpers.

use omni_core::js::is_js_whitespace;
use serde::{Deserialize, Deserializer};

/// A Markdown code block whose fence does not collide with the content.
pub fn code_block(content: &str, lang: Option<&str>) -> String {
    let mut fence = "```".to_owned();
    while content.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}{}\n{content}\n{fence}", lang.unwrap_or(""))
}

/// `#[serde(default, deserialize_with = "present")]`: an absent field is
/// `None`, but an explicit `null` is rejected.
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
    fn code_block_extends_fence() {
        assert_eq!(code_block("a", None), "```\na\n```");
        assert_eq!(code_block("```", Some("json")), "````json\n```\n````");
    }
}
