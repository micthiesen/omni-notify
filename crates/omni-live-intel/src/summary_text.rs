//! Cleanup of model-written summaries and topic labels (`summaryText.ts`).
//!
//! Lengths and cut points use UTF-16 code units, as JS strings do.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use icu_normalizer::ComposingNormalizerBorrowed;
use omni_core::js::{utf16_len, utf16_slice};
use regex::Regex;

use crate::js_math::is_js_whitespace;

const SUMMARY_MAX_CHARS: usize = 220;
const TOPIC_MAX_CHARS: usize = 60;
const SUMMARY_FALLBACK: &str = "The current discussion could not be summarized cleanly.";
const TOPIC_FALLBACK: &str = "Current discussion";

const TOPIC_STOP_WORDS: [&str; 17] = [
    "a", "about", "and", "as", "at", "by", "for", "from", "in", "into", "of", "on", "over", "the",
    "to", "versus", "with",
];

fn regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        // The patterns are literals covered by tests.
        Err(e) => unreachable!("invalid summary-text regex {pattern}: {e}"),
    }
}

static CONTROL_OR_FORMAT: LazyLock<Regex> = LazyLock::new(|| regex(r"[\p{Cc}\p{Cf}]"));
static WHITESPACE_RUN: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+"));
static PIPE_HASH_TAIL: LazyLock<Regex> = LazyLock::new(|| regex(r"[|#]{2,}$"));
static FOREIGN_SCRIPT_TAIL: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\s*\S*[\p{Script=Han}\p{Script=Thai}\p{Script=Bengali}]\S*$"));
static PUNCTUATION_TAIL: LazyLock<Regex> = LazyLock::new(|| regex(r"[,:;\-–—]+$"));
static COMPLETE_ENDING: LazyLock<Regex> = LazyLock::new(|| regex(r#"[.!?…]["'”’)]?$"#));
static NON_TOKEN: LazyLock<Regex> = LazyLock::new(|| regex(r"[^a-z0-9 ]"));

fn normalize_spacing(value: &str) -> String {
    let nfkc = ComposingNormalizerBorrowed::new_nfkc().normalize(value);
    let stripped = CONTROL_OR_FORMAT.replace_all(&nfkc, "");
    WHITESPACE_RUN
        .replace_all(&stripped, " ")
        .trim_matches(is_js_whitespace)
        .to_owned()
}

fn strip_constrained_output_artifact(value: &str) -> String {
    let without_pipes = PIPE_HASH_TAIL.replacen(value, 1, "");
    FOREIGN_SCRIPT_TAIL
        .replacen(&without_pipes, 1, "")
        .trim_matches(is_js_whitespace)
        .to_owned()
}

fn strip_punctuation_tail(value: &str) -> String {
    PUNCTUATION_TAIL
        .replacen(value, 1, "")
        .trim_end_matches(is_js_whitespace)
        .to_owned()
}

/// `Math.floor(n * fraction)` with JS double arithmetic.
#[allow(clippy::cast_precision_loss)]
fn js_floor_fraction(n: usize, fraction: f64) -> f64 {
    (n as f64 * fraction).floor()
}

/// UTF-16 offset of byte index `byte` in `s`.
fn utf16_offset(s: &str, byte: usize) -> usize {
    s.get(..byte).map_or(0, utf16_len)
}

fn truncate_at_word(value: &str, max_chars: usize) -> String {
    if utf16_len(value) <= max_chars {
        return value.to_owned();
    }
    let candidate = utf16_slice(value, 0, max_chars - 1)
        .trim_end_matches(is_js_whitespace)
        .to_owned();
    let cut = match candidate.rfind(' ') {
        Some(byte)
            if utf16_offset(&candidate, byte) as f64 >= js_floor_fraction(max_chars, 0.6) =>
        {
            candidate[..byte].to_owned()
        }
        _ => candidate,
    };
    format!("{}…", strip_punctuation_tail(&cut))
}

fn is_closing_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | ')')
}

/// End byte offsets of `/[.!?]["'”’)]?(?=\s|$)/gu` matches.
fn sentence_ends(value: &str) -> Vec<usize> {
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    let boundary = |index: usize| chars.get(index).is_none_or(|(_, c)| is_js_whitespace(*c));
    let end_of = |index: usize| chars.get(index).map_or(value.len(), |(byte, _)| *byte);
    let mut ends = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let (_, c) = chars[index];
        if matches!(c, '.' | '!' | '?') {
            let with_quote = chars
                .get(index + 1)
                .is_some_and(|(_, next)| is_closing_quote(*next))
                && boundary(index + 2);
            if with_quote {
                ends.push(end_of(index + 2));
                index += 2;
                continue;
            }
            if boundary(index + 1) {
                ends.push(end_of(index + 1));
                index += 1;
                continue;
            }
        }
        index += 1;
    }
    ends
}

/// Keeps the summary below its schema ceiling and turns a constrained-decoding
/// tail into a clean sentence or an explicit ellipsis.
pub fn clean_livestream_summary(value: &str) -> String {
    let cleaned = strip_constrained_output_artifact(&normalize_spacing(value));
    if cleaned.is_empty() {
        return SUMMARY_FALLBACK.to_owned();
    }
    let cleaned = truncate_at_word(&cleaned, SUMMARY_MAX_CHARS);
    if COMPLETE_ENDING.is_match(&cleaned) {
        return cleaned;
    }
    if let Some(end) = sentence_ends(&cleaned).last().copied()
        && utf16_offset(&cleaned, end) as f64 >= js_floor_fraction(utf16_len(&cleaned), 0.45)
    {
        return cleaned[..end].to_owned();
    }
    format!("{}…", strip_punctuation_tail(&cleaned))
}

/// A compact topic label, or `"Current discussion"` when nothing usable remains.
pub fn clean_livestream_topic(value: &str) -> String {
    let stripped = strip_constrained_output_artifact(&normalize_spacing(value));
    let cleaned = PUNCTUATION_TAIL.replacen(&stripped, 1, "").into_owned();
    if cleaned.is_empty() {
        TOPIC_FALLBACK.to_owned()
    } else {
        truncate_at_word(&cleaned, TOPIC_MAX_CHARS)
    }
}

fn strip_token_suffix(token: &str) -> &str {
    if let Some(stem) = token.strip_suffix("ing") {
        stem
    } else if let Some(stem) = token
        .strip_suffix("ed")
        .or_else(|| token.strip_suffix("es"))
    {
        stem
    } else {
        token.strip_suffix('s').unwrap_or(token)
    }
}

fn topic_tokens(value: &str) -> BTreeSet<String> {
    let lowered = normalize_spacing(value).to_lowercase();
    let replaced = NON_TOKEN.replace_all(&lowered, " ");
    let collapsed = WHITESPACE_RUN.replace_all(&replaced, " ");
    collapsed
        .trim_matches(is_js_whitespace)
        .split(' ')
        .map(strip_token_suffix)
        .filter(|token| token.len() >= 3 && !TOPIC_STOP_WORDS.contains(token))
        .map(str::to_owned)
        .collect()
}

/// Broader than exact labels, while still requiring shared subject words.
pub fn are_same_livestream_topic(a: &str, b: &str) -> bool {
    let left_text = normalize_spacing(a).to_lowercase();
    let right_text = normalize_spacing(b).to_lowercase();
    if left_text == right_text {
        return true;
    }
    if utf16_len(&left_text).min(utf16_len(&right_text)) >= 12
        && (left_text.contains(&right_text) || right_text.contains(&left_text))
    {
        return true;
    }
    let left = topic_tokens(a);
    let right = topic_tokens(b);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let shared = left.intersection(&right).count();
    let union = left.union(&right).count();
    #[allow(clippy::cast_precision_loss)]
    let containment = shared as f64 / left.len().min(right.len()) as f64;
    #[allow(clippy::cast_precision_loss)]
    let overlap = shared as f64 / union as f64;
    shared >= 2 && (containment >= 0.5 || overlap >= 0.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_ends_follow_the_js_lookahead() {
        assert_eq!(sentence_ends("A. B"), vec![2]);
        assert_eq!(sentence_ends("Say \"hi.\" now"), vec![9]);
        assert_eq!(sentence_ends("v1.2 is out!"), vec![12]);
        assert!(sentence_ends("x.\"y").is_empty());
    }

    #[test]
    fn suffixes_strip_like_the_leftmost_regex_match() {
        assert_eq!(strip_token_suffix("boxes"), "box");
        assert_eq!(strip_token_suffix("using"), "us");
        assert_eq!(strip_token_suffix("benefits"), "benefit");
        assert_eq!(strip_token_suffix("risked"), "risk");
    }
}
