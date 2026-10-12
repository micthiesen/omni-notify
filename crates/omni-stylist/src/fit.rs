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

/// What a piece's recorded history says about its fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FitEvidence {
    /// Only positive fit signals: fitting try-ons, acceptable wear or
    /// feedback, a matched outcome, or a `baseline` reference role.
    Proven,
    /// Only adverse signals: a `too_*` region, a wrong-size try-on, a missed
    /// outcome, a fit-related return, a too-small or too-large reference role,
    /// or a wash that shrank or stretched it.
    Failed,
    /// Both.
    Mixed,
    /// Owned or bought but never assessed.
    Untested,
}

/// The fit evidence of one piece, with the ids behind it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PieceEvidence {
    pub piece_id: String,
    pub fit: FitEvidence,
    pub positive: Vec<String>,
    pub adverse: Vec<String>,
}

/// Classifies a piece's current (non-superseded) history.
pub fn piece_fit_evidence(state: &LedgerState, piece_id: &str) -> PieceEvidence {
    use crate::model::{KeepDecision, OutcomeResult, ReasonAspect, ReferenceRole, TryOnOverall};
    let mut positive = Vec::new();
    let mut adverse = Vec::new();
    if let Some(reference) = state.piece(piece_id).and_then(|p| p.reference.as_ref()) {
        match reference.role {
            ReferenceRole::Baseline => positive.push(format!("{piece_id}:reference_baseline")),
            ReferenceRole::TooSmall | ReferenceRole::TooLarge => {
                adverse.push(format!(
                    "{piece_id}:reference_{}",
                    crate::state::enum_text(&reference.role)
                ));
            }
            ReferenceRole::Partial => {}
        }
    }
    for event in state.piece_events(piece_id) {
        let id = event.id.as_str().to_owned();
        let body = &event.envelope.event;
        let fits = body.region_fit();
        let any_adverse = fits.iter().any(|f| is_adverse(f.verdict));
        match body {
            EventBody::TryOn(t) => {
                if any_adverse || t.overall == TryOnOverall::WrongSize {
                    adverse.push(id);
                } else if t.overall == TryOnOverall::Fits {
                    positive.push(id);
                }
            }
            EventBody::Wear(_) | EventBody::Feedback(_) if !fits.is_empty() => {
                if any_adverse {
                    adverse.push(id);
                } else {
                    positive.push(id);
                }
            }
            EventBody::Outcome(o) => match o.result {
                OutcomeResult::Missed => adverse.push(id),
                OutcomeResult::Matched if !any_adverse => positive.push(id),
                _ if any_adverse => adverse.push(id),
                _ => {}
            },
            EventBody::Decision(d)
                if d.decision == KeepDecision::Return
                    && d.reasons.iter().any(|r| r.aspect == ReasonAspect::Fit) =>
            {
                adverse.push(id);
            }
            EventBody::Wash(w)
                if w.observed_changes
                    .iter()
                    .any(|c| matches!(c.change, ChangeKind::Shrank | ChangeKind::Stretched)) =>
            {
                adverse.push(id);
            }
            _ => {}
        }
    }
    let fit = match (positive.is_empty(), adverse.is_empty()) {
        (false, true) => FitEvidence::Proven,
        (true, false) => FitEvidence::Failed,
        (false, false) => FitEvidence::Mixed,
        (true, true) => FitEvidence::Untested,
    };
    PieceEvidence {
        piece_id: piece_id.to_owned(),
        fit,
        positive,
        adverse,
    }
}

/// Whether two recorded attributes agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Agreement {
    Same,
    Differs,
    /// Recorded on only one side, or (for material) on neither.
    Unrecorded,
}

/// A prior piece with the candidate's brand, model, cut, size label and size system.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityMatch {
    pub piece_id: String,
    pub variant_id: String,
    pub version: Agreement,
    pub material: Agreement,
    pub fit: FitEvidence,
}

/// What the ledger supports for a candidate, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BasisAssessment {
    pub basis: Basis,
    pub reasons: Vec<String>,
    pub identity_matches: Vec<IdentityMatch>,
    /// Identity matches with the same version and material whose fit failed.
    pub known_failures: Vec<String>,
    /// Pieces of the brand with any recorded fit evidence.
    pub brand_evidence_pieces: Vec<String>,
}

fn version_agreement(a: Option<&String>, b: Option<&String>) -> Agreement {
    match (a, b) {
        (Some(a), Some(b)) if same_text(a, b) => Agreement::Same,
        (Some(_), Some(_)) => Agreement::Differs,
        (None, None) => Agreement::Same,
        _ => Agreement::Unrecorded,
    }
}

/// Same fibers within 5 percentage points each, and the same construction
/// when both record it. Material must be recorded on both sides to agree.
fn material_agreement(a: &VariantRecord, b: &VariantRecord) -> Agreement {
    let (Some(fa), Some(fb)) = (&a.fabric, &b.fabric) else {
        return Agreement::Unrecorded;
    };
    if fa.composition.is_empty() || fb.composition.is_empty() {
        return Agreement::Unrecorded;
    }
    let share = |fabric: &crate::model::Fabric, fiber: &str| {
        fabric
            .composition
            .iter()
            .filter(|s| s.fiber.as_str() == fiber)
            .map(|s| s.percent)
            .sum::<f64>()
    };
    let fibers: std::collections::BTreeSet<&str> = fa
        .composition
        .iter()
        .chain(&fb.composition)
        .map(|s| s.fiber.as_str())
        .collect();
    let close = fibers
        .iter()
        .all(|fiber| (share(fa, fiber) - share(fb, fiber)).abs() <= 5.0);
    let construction = match (fa.construction, fb.construction) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    };
    if close && construction {
        Agreement::Same
    } else {
        Agreement::Differs
    }
}

/// The strongest basis the ledger supports for `candidate`:
///
/// - `exact_repeat`: a prior piece of the same brand, model, cut, size label
///   and size system, with the same version (or none recorded on either) and
///   the same fiber composition, has proven fit, and no such piece has
///   adverse fit evidence;
/// - `known_brand_new_cut`: some piece of the brand has recorded fit evidence;
/// - `new_brand_inference`: otherwise. Owning an untested piece is not evidence.
pub fn assess_basis(state: &LedgerState, candidate: &VariantRecord) -> BasisAssessment {
    let mut matches = Vec::new();
    let mut brand_evidence = Vec::new();
    let mut reasons = Vec::new();
    for record in state.current_records() {
        if !matches!(record.body, crate::model::RecordBody::Piece(_)) {
            continue;
        }
        let piece_id = record.id.as_str();
        let Some((variant_id, variant)) = state.piece_variant(piece_id) else {
            continue;
        };
        if !same_text(&variant.brand, &candidate.brand) {
            continue;
        }
        let evidence = piece_fit_evidence(state, piece_id);
        if evidence.fit != FitEvidence::Untested {
            brand_evidence.push(piece_id.to_owned());
        }
        let same_identity = same_text(&variant.model, &candidate.model)
            && same_opt(variant.cut.as_ref(), candidate.cut.as_ref())
            && same_text(&variant.size_label, &candidate.size_label)
            && variant.size_system == candidate.size_system;
        if same_identity {
            let found = IdentityMatch {
                piece_id: piece_id.to_owned(),
                variant_id: variant_id.to_owned(),
                version: version_agreement(variant.version.as_ref(), candidate.version.as_ref()),
                material: material_agreement(variant, candidate),
                fit: evidence.fit,
            };
            match found.version {
                Agreement::Differs => reasons.push(format!(
                    "{piece_id} is the same model and size but version {} differs from {}",
                    variant.version.as_deref().unwrap_or("?"),
                    candidate.version.as_deref().unwrap_or("?")
                )),
                Agreement::Unrecorded => reasons.push(format!(
                    "{piece_id}: the version is recorded on only one of the two variants"
                )),
                Agreement::Same => {}
            }
            match found.material {
                Agreement::Differs => {
                    reasons.push(format!("{piece_id}: the fiber composition differs"));
                }
                Agreement::Unrecorded => reasons.push(format!(
                    "{piece_id}: fiber composition is not recorded on both variants"
                )),
                Agreement::Same => {}
            }
            match found.fit {
                FitEvidence::Untested => reasons.push(format!(
                    "{piece_id} is the same variant but has no recorded fit (owned is not proven)"
                )),
                FitEvidence::Failed => {
                    reasons.push(format!("{piece_id} is the same variant and its fit failed"));
                }
                FitEvidence::Mixed => reasons.push(format!(
                    "{piece_id} is the same variant with both positive and adverse fit evidence"
                )),
                FitEvidence::Proven => {}
            }
            matches.push(found);
        }
    }
    let compatible =
        |m: &&IdentityMatch| m.version == Agreement::Same && m.material == Agreement::Same;
    let known_failures: Vec<String> = matches
        .iter()
        .filter(compatible)
        .filter(|m| m.fit == FitEvidence::Failed)
        .map(|m| m.piece_id.clone())
        .collect();
    let proven = matches
        .iter()
        .filter(compatible)
        .any(|m| m.fit == FitEvidence::Proven);
    let contradicted = matches
        .iter()
        .filter(compatible)
        .any(|m| matches!(m.fit, FitEvidence::Failed | FitEvidence::Mixed));
    let basis = if proven && !contradicted {
        Basis::ExactRepeat
    } else if !brand_evidence.is_empty() {
        Basis::KnownBrandNewCut
    } else {
        if !matches.is_empty()
            || state.current_records().any(|r| {
                state
                    .piece_variant(r.id.as_str())
                    .is_some_and(|(_, v)| same_text(&v.brand, &candidate.brand))
            })
        {
            reasons.push(format!(
                "no piece of {} has recorded fit evidence",
                candidate.brand
            ));
        }
        Basis::NewBrandInference
    };
    BasisAssessment {
        basis,
        reasons,
        identity_matches: matches,
        known_failures,
        brand_evidence_pieces: brand_evidence,
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
