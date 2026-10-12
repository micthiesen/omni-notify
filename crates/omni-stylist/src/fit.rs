//! Pure fit decisions: comparing reported fit with frozen expectations, the
//! evidence basis of a candidate, and which fit cases a rule's scope covers.

use serde::{Deserialize, Serialize};

use crate::model::{
    Basis, ChangeKind, EventBody, FitVerdict, RegionExpectation, RegionFit, RuleScope,
    VariantRecord,
};
use crate::state::{Event, LedgerState};

/// Width or length, with a signed level (-2 too small ... +2 too large).
fn axis_level(verdict: FitVerdict) -> Option<(Option<Axis>, i8)> {
    use FitVerdict::*;
    match verdict {
        Good => Some((None, 0)),
        TooTight => Some((Some(Axis::Width), -2)),
        SlightlyTight => Some((Some(Axis::Width), -1)),
        SlightlyLoose => Some((Some(Axis::Width), 1)),
        TooLoose => Some((Some(Axis::Width), 2)),
        TooShort => Some((Some(Axis::Length), -2)),
        SlightlyShort => Some((Some(Axis::Length), -1)),
        SlightlyLong => Some((Some(Axis::Length), 1)),
        TooLong => Some((Some(Axis::Length), 2)),
        Unknown => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Width,
    Length,
}

/// How a reported region compares with its frozen expectation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegionMatch {
    /// Same verdict.
    Exact,
    /// One step apart on the same axis (`good` against `slightly_tight`).
    Close,
    /// Two or more steps apart, or the opposite axis.
    Miss,
    /// Either side is `unknown`.
    Unknown,
    /// Expected but not reported.
    NotReported,
    /// Reported but never forecast.
    NotForecast,
}

pub fn match_verdict(expected: FitVerdict, actual: FitVerdict) -> RegionMatch {
    if expected == actual {
        return RegionMatch::Exact;
    }
    let (Some((expected_axis, expected_level)), Some((actual_axis, actual_level))) =
        (axis_level(expected), axis_level(actual))
    else {
        return RegionMatch::Unknown;
    };
    if let (Some(a), Some(b)) = (expected_axis, actual_axis)
        && a != b
    {
        return RegionMatch::Miss;
    }
    if (expected_level - actual_level).abs() == 1 {
        RegionMatch::Close
    } else {
        RegionMatch::Miss
    }
}

/// One region of a forecast-against-report comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionComparison {
    pub region: String,
    pub expected: Option<FitVerdict>,
    pub actual: Option<FitVerdict>,
    #[serde(rename = "match")]
    pub matched: RegionMatch,
}

/// Compares reported region fit with the frozen expectations.
pub fn compare_regions(
    expectations: &[RegionExpectation],
    reported: &[RegionFit],
) -> Vec<RegionComparison> {
    let mut out: Vec<RegionComparison> = expectations
        .iter()
        .map(|expectation| {
            let actual = reported
                .iter()
                .find(|fit| fit.region == expectation.region)
                .map(|fit| fit.verdict);
            RegionComparison {
                region: expectation.region.as_str().to_owned(),
                expected: Some(expectation.expected),
                actual,
                matched: actual.map_or(RegionMatch::NotReported, |actual| {
                    match_verdict(expectation.expected, actual)
                }),
            }
        })
        .collect();
    for fit in reported {
        if !expectations.iter().any(|e| e.region == fit.region) {
            out.push(RegionComparison {
                region: fit.region.as_str().to_owned(),
                expected: None,
                actual: Some(fit.verdict),
                matched: RegionMatch::NotForecast,
            });
        }
    }
    out
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn same_opt(a: Option<&String>, b: Option<&String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same_text(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// What the ledger supports for `candidate`: an exact repeat when a piece of
/// the same brand, model, cut and size was owned or tried (any status), a
/// known brand when any piece of the brand was, otherwise a new brand.
/// Returns the supporting piece ids.
pub fn basis_for(state: &LedgerState, candidate: &VariantRecord) -> (Basis, Vec<String>) {
    let mut exact = Vec::new();
    let mut brand = Vec::new();
    for record in state.current_records() {
        if !matches!(record.body, crate::model::RecordBody::Piece(_)) {
            continue;
        }
        let Some((_, variant)) = state.piece_variant(record.id.as_str()) else {
            continue;
        };
        if !same_text(&variant.brand, &candidate.brand) {
            continue;
        }
        brand.push(record.id.as_str().to_owned());
        if same_text(&variant.model, &candidate.model)
            && same_opt(variant.cut.as_ref(), candidate.cut.as_ref())
            && same_text(&variant.size_label, &candidate.size_label)
        {
            exact.push(record.id.as_str().to_owned());
        }
    }
    if !exact.is_empty() {
        (Basis::ExactRepeat, exact)
    } else if !brand.is_empty() {
        (Basis::KnownBrandNewCut, brand)
    } else {
        (Basis::NewBrandInference, Vec::new())
    }
}

/// Whether `variant` falls inside `scope`.
pub fn variant_in_scope(scope: &RuleScope, variant: &VariantRecord) -> bool {
    scope
        .brand
        .as_ref()
        .is_none_or(|b| same_text(b, &variant.brand))
        && scope
            .model
            .as_ref()
            .is_none_or(|m| same_text(m, &variant.model))
        && scope
            .category
            .as_ref()
            .is_none_or(|c| variant.category.is_under(c.as_str()))
        && scope
            .size_label
            .as_ref()
            .is_none_or(|s| same_text(s, &variant.size_label))
        && scope.size_system.is_none_or(|s| s == variant.size_system)
        && scope.fiber.as_ref().is_none_or(|fiber| {
            variant
                .fabric
                .as_ref()
                .is_some_and(|f| f.composition.iter().any(|share| share.fiber == *fiber))
        })
}

/// The current fit cases inside `scope`: try-ons, outcomes, fit-reporting
/// wear and feedback, and washes that changed something, for pieces or
/// predictions whose variant is in scope (and that report the scoped region).
pub fn cases_in_scope<'a>(state: &'a LedgerState, scope: &'a RuleScope) -> Vec<&'a Event> {
    state
        .current_events()
        .filter(|event| event.envelope.event.is_fit_case())
        .filter(|event| {
            let body = &event.envelope.event;
            let subject = body
                .piece_id()
                .or_else(|| body.prediction_id())
                .map(|id| id.as_str());
            let Some((_, variant)) = subject.and_then(|id| state.variant_of(id)) else {
                return false;
            };
            variant_in_scope(scope, variant) && region_in_scope(scope, body)
        })
        .collect()
}

fn region_in_scope(scope: &RuleScope, body: &EventBody) -> bool {
    let Some(region) = &scope.region else {
        return true;
    };
    if body.region_fit().iter().any(|fit| fit.region == *region) {
        return true;
    }
    match body {
        EventBody::Wash(wash) => wash
            .observed_changes
            .iter()
            .any(|change| change.region == *region && change.change != ChangeKind::NoChange),
        _ => false,
    }
}

/// Whether a verdict reports a clear misfit (`too_*`).
pub fn is_adverse(verdict: FitVerdict) -> bool {
    matches!(
        verdict,
        FitVerdict::TooTight | FitVerdict::TooLoose | FitVerdict::TooShort | FitVerdict::TooLong
    )
}

/// Whether a verdict reports a slight deviation (`slightly_*`).
pub fn is_slight(verdict: FitVerdict) -> bool {
    matches!(
        verdict,
        FitVerdict::SlightlyTight
            | FitVerdict::SlightlyLoose
            | FitVerdict::SlightlyShort
            | FitVerdict::SlightlyLong
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Slug;
    use FitVerdict::*;

    #[test]
    fn verdicts_compare_on_their_axis() {
        assert_eq!(match_verdict(Good, Good), RegionMatch::Exact);
        assert_eq!(match_verdict(Good, SlightlyTight), RegionMatch::Close);
        assert_eq!(match_verdict(Good, TooShort), RegionMatch::Miss);
        assert_eq!(
            match_verdict(SlightlyLoose, SlightlyTight),
            RegionMatch::Miss
        );
        assert_eq!(
            match_verdict(SlightlyTight, SlightlyShort),
            RegionMatch::Miss
        );
        assert_eq!(match_verdict(Good, Unknown), RegionMatch::Unknown);
    }

    #[test]
    fn region_comparison_lists_unforecast_and_unreported_regions() {
        let expectations = vec![RegionExpectation {
            region: Slug("chest".to_owned()),
            expected: Good,
            rationale: "reference".to_owned(),
            evidence_refs: Vec::new(),
        }];
        let reported = vec![RegionFit {
            region: Slug("body_length".to_owned()),
            verdict: TooShort,
            note: None,
        }];
        let comparison = compare_regions(&expectations, &reported);
        assert_eq!(comparison[0].matched, RegionMatch::NotReported);
        assert_eq!(comparison[1].matched, RegionMatch::NotForecast);
    }
}
