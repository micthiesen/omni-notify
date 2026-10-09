//! Content-completeness scoring for a synthesized chunk
//! (`src/press-pods/speech/coverage.ts`).
//!
//! Higgs silently truncates, emitting a natural-sounding read of only the
//! first part of a chunk, which a duration check cannot reliably catch (a
//! truncated read and a fast read overlap in seconds per char). Word coverage
//! of an STT transcript separates them. Pure and deterministic.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

static NON_WORD: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[^a-z0-9\s]").ok());
static SPACES: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\s+").ok());

/// Comparable word tokens (case and punctuation insensitive).
fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let words = NON_WORD
        .as_ref()
        .map(|re| re.replace_all(&lower, " ").into_owned())
        .unwrap_or(lower);
    let collapsed = SPACES
        .as_ref()
        .map(|re| re.replace_all(&words, " ").into_owned())
        .unwrap_or(words);
    collapsed
        .trim()
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Transcript coverage of the expected text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverageResult {
    /// Fraction of distinct expected words present in the transcript.
    pub coverage: f64,
    /// Transcript words / expected words: ~1 complete, well below 1 for a
    /// truncation, well above 1 for a runaway loop.
    pub word_ratio: f64,
    pub expected_words: usize,
    pub transcript_words: usize,
}

#[allow(clippy::cast_precision_loss)]
pub fn compute_coverage(expected: &str, transcript: &str) -> CoverageResult {
    let expected_tokens = tokenize(expected);
    let transcript_tokens = tokenize(transcript);
    if expected_tokens.is_empty() {
        return CoverageResult {
            coverage: 1.0,
            word_ratio: 1.0,
            expected_words: 0,
            transcript_words: 0,
        };
    }
    let expected_set: HashSet<&String> = expected_tokens.iter().collect();
    let transcript_set: HashSet<&String> = transcript_tokens.iter().collect();
    let present = expected_set
        .iter()
        .filter(|w| transcript_set.contains(*w))
        .count();
    CoverageResult {
        coverage: present as f64 / expected_set.len() as f64,
        word_ratio: transcript_tokens.len() as f64 / expected_tokens.len() as f64,
        expected_words: expected_tokens.len(),
        transcript_words: transcript_tokens.len(),
    }
}

/// Acceptance thresholds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentBounds {
    /// Minimum word coverage for a complete read.
    pub min_coverage: f64,
    /// Lower coverage floor when the transcript length shows the whole read is
    /// present (STT writing spoken numbers as digits).
    pub min_coverage_with_healthy_ratio: f64,
    /// Minimum word ratio required to use the lower floor.
    pub min_healthy_word_ratio: f64,
    /// Maximum input size for the number-heavy allowance.
    pub max_healthy_ratio_expected_words: usize,
    /// Maximum word ratio before a read is a runaway loop.
    pub max_word_ratio: f64,
}

/// Measured separation (Higgs): complete reads ~1.0 coverage and ratio;
/// truncations at most 0.66 coverage and 0.47 ratio; number-heavy complete
/// reads down to 0.68 coverage with at least a 0.78 ratio.
pub const DEFAULT_CONTENT_BOUNDS: ContentBounds = ContentBounds {
    min_coverage: 0.75,
    min_coverage_with_healthy_ratio: 0.68,
    min_healthy_word_ratio: 0.78,
    max_healthy_ratio_expected_words: 60,
    max_word_ratio: 1.8,
};

pub fn is_content_complete(result: &CoverageResult) -> bool {
    is_content_complete_with(result, &DEFAULT_CONTENT_BOUNDS)
}

pub fn is_content_complete_with(result: &CoverageResult, bounds: &ContentBounds) -> bool {
    if result.word_ratio > bounds.max_word_ratio {
        return false;
    }
    result.coverage >= bounds.min_coverage
        || (result.coverage >= bounds.min_coverage_with_healthy_ratio
            && result.word_ratio >= bounds.min_healthy_word_ratio
            && result.expected_words <= bounds.max_healthy_ratio_expected_words)
}

#[cfg(test)]
mod coverage_spec {
    //! Ports `src/press-pods/speech/coverage.spec.ts`.
    use super::*;

    const COMPLETE: &str = "I strongly believe that written text should be from humans to
humans. Yet, I still write my texts with the help of language models, and I
don't find that contradictory. What distinguishes AI slop from good writing is
whether a human has put thought behind it. You cannot outsource thinking.";

    fn result(
        coverage: f64,
        word_ratio: f64,
        expected_words: usize,
        transcript_words: usize,
    ) -> CoverageResult {
        CoverageResult {
            coverage,
            word_ratio,
            expected_words,
            transcript_words,
        }
    }

    #[test]
    fn scores_an_exact_transcript_as_full_coverage() {
        let r = compute_coverage(COMPLETE, COMPLETE);
        assert_eq!(r.coverage, 1.0);
        assert!((r.word_ratio - 1.0).abs() < 1e-5);
    }

    #[test]
    fn tolerates_casing_and_punctuation_differences() {
        let stt = COMPLETE.replace("AI", "A.I.").to_uppercase();
        assert!(compute_coverage(COMPLETE, &stt).coverage >= 0.9);
    }

    #[test]
    fn detects_truncation_as_low_coverage() {
        let words: Vec<&str> = COMPLETE.split_whitespace().collect();
        let truncated = words[..(words.len() * 4 / 10)].join(" ");
        let r = compute_coverage(COMPLETE, &truncated);
        assert!(r.coverage < 0.6);
        assert!(r.word_ratio < 0.6);
        assert!(!is_content_complete(&r));
    }

    #[test]
    fn passes_a_complete_read_under_the_default_bounds() {
        assert!(is_content_complete(&compute_coverage(COMPLETE, COMPLETE)));
    }

    #[test]
    fn flags_a_runaway_loop_via_word_ratio_even_at_full_coverage() {
        let looped = format!("{COMPLETE} {COMPLETE} {COMPLETE}");
        let r = compute_coverage(COMPLETE, &looped);
        assert_eq!(r.coverage, 1.0);
        assert!(r.word_ratio > DEFAULT_CONTENT_BOUNDS.max_word_ratio);
        assert!(!is_content_complete(&r));
    }

    #[test]
    fn treats_empty_expected_text_as_complete() {
        let r = compute_coverage("", "anything");
        assert_eq!(r.coverage, 1.0);
        assert!(is_content_complete(&r));
    }

    #[test]
    fn reflects_the_real_truncated_chunk_measurements() {
        assert!(!is_content_complete(&result(0.48, 0.36, 146, 52)));
        assert!(!is_content_complete(&result(0.66, 0.47, 119, 56)));
        assert!(is_content_complete(&result(1.0, 1.0, 51, 52)));
    }

    #[test]
    fn accepts_production_number_heavy_reads_with_healthy_transcript_lengths() {
        for r in [
            result(0.71, 0.83, 59, 49),
            result(0.68, 0.78, 32, 25),
            result(0.72, 0.84, 38, 32),
        ] {
            assert!(is_content_complete(&r), "{r:?}");
        }
    }

    #[test]
    fn does_not_let_the_secondary_coverage_pass_admit_true_truncations() {
        assert!(!is_content_complete(&result(
            DEFAULT_CONTENT_BOUNDS.min_coverage_with_healthy_ratio,
            DEFAULT_CONTENT_BOUNDS.min_healthy_word_ratio - 0.01,
            100,
            69
        )));
    }

    #[test]
    fn does_not_relax_coverage_for_long_chunks() {
        assert!(!is_content_complete(&result(
            0.7,
            0.8,
            DEFAULT_CONTENT_BOUNDS.max_healthy_ratio_expected_words + 1,
            49
        )));
    }
}
