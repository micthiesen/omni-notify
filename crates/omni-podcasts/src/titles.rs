//! Loose title normalization shared by every podcast matching step
//! (`src/podcast-recs/titles.ts`).

use std::sync::LazyLock;

use icu_normalizer::DecomposingNormalizerBorrowed;
use regex::Regex;

static NON_ALPHANUMERIC: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").ok());

/// Lowercase, strip diacritics and apostrophes, replace remaining punctuation
/// with spaces, collapse whitespace.
///
/// Outputs are only ever compared against other outputs of this function within
/// one run (never persisted), so the implementation may evolve.
pub fn normalize_title(title: &str) -> String {
    let decomposed = DecomposingNormalizerBorrowed::new_nfkd().normalize(title);
    let stripped: String = decomposed
        .chars()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .collect();
    let lowered: String = stripped
        .to_lowercase()
        .chars()
        .filter(|c| *c != '\'' && *c != '\u{2019}')
        .collect();
    let spaced = match NON_ALPHANUMERIC.as_ref() {
        Some(re) => re.replace_all(&lowered, " ").into_owned(),
        None => lowered,
    };
    spaced.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_diacritics_apostrophes_and_punctuation() {
        assert_eq!(
            normalize_title("Séan Carroll's Mindscape!"),
            normalize_title("sean carrolls mindscape")
        );
        assert_eq!(
            normalize_title("The  Rest   Is History"),
            "the rest is history"
        );
        assert_eq!(normalize_title("Radio-Lab!!"), "radio lab");
        assert_eq!(normalize_title("Café Society"), "cafe society");
        assert_eq!(normalize_title("  "), "");
    }
}
