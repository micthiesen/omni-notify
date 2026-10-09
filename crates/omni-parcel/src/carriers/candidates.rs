//! Ranked carrier candidates (`src/parcel-tracker/carriers/candidates.ts`).

use std::collections::HashSet;

pub const MAX_CARRIER_CANDIDATES: usize = 3;

/// Valid candidates (ranked, deduped, capped) and those Parcel does not know.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CandidateSelection {
    pub valid: Vec<String>,
    pub invalid: Vec<String>,
}

/// `selectValidCandidates`: splits ranked candidates against the live carrier
/// list, preserving rank order and dropping duplicates and blanks.
pub fn select_valid_candidates(
    candidates: &[String],
    valid_codes: &HashSet<String>,
) -> CandidateSelection {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut selection = CandidateSelection::default();
    for raw in candidates {
        let code = omni_email::sender_rules::js_trim(raw);
        if code.is_empty() || !seen.insert(code) {
            continue;
        }
        if valid_codes.contains(code) {
            selection.valid.push(code.to_owned());
        } else {
            selection.invalid.push(code.to_owned());
        }
    }
    selection.valid.truncate(MAX_CARRIER_CANDIDATES);
    selection
}

#[cfg(test)]
mod candidates_spec {
    use super::*;

    fn valid_codes() -> HashSet<String> {
        ["dicom", "gls", "ups", "canpost", "fedex"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn select(candidates: &[&str]) -> CandidateSelection {
        let owned: Vec<String> = candidates.iter().map(|c| (*c).to_owned()).collect();
        select_valid_candidates(&owned, &valid_codes())
    }

    #[test]
    fn keeps_valid_candidates_in_ranked_order() {
        let result = select(&["dicom", "gls", "canpost"]);
        assert_eq!(result.valid, ["dicom", "gls", "canpost"]);
        assert!(result.invalid.is_empty());
    }

    #[test]
    fn splits_out_invalid_candidates_without_breaking_rank_order() {
        let result = select(&["bogus", "dicom", "gls"]);
        assert_eq!(result.valid, ["dicom", "gls"]);
        assert_eq!(result.invalid, ["bogus"]);
    }

    #[test]
    fn returns_empty_valid_list_when_nothing_matches() {
        let result = select(&["nope", "nada"]);
        assert!(result.valid.is_empty());
        assert_eq!(result.invalid, ["nope", "nada"]);
    }

    #[test]
    fn dedupes_repeated_candidates_keeping_the_first_occurrence() {
        assert_eq!(select(&["dicom", "gls", "dicom"]).valid, ["dicom", "gls"]);
    }

    #[test]
    fn trims_whitespace_and_drops_blank_entries() {
        let result = select(&[" dicom ", "", "  "]);
        assert_eq!(result.valid, ["dicom"]);
        assert!(result.invalid.is_empty());
    }

    #[test]
    fn caps_valid_candidates_at_max_carrier_candidates() {
        let result = select(&["dicom", "gls", "ups", "canpost", "fedex"]);
        assert_eq!(result.valid.len(), MAX_CARRIER_CANDIDATES);
        assert_eq!(result.valid, ["dicom", "gls", "ups"]);
    }

    #[test]
    fn handles_empty_input() {
        assert_eq!(select(&[]), CandidateSelection::default());
    }
}
