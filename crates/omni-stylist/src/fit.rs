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
    /// The direction of the latest report, by date then ledger order.
    pub latest: Option<Signal>,
    pub positive: Vec<String>,
    pub adverse: Vec<String>,
}

/// Classifies a piece's current (non-superseded) history.
pub fn piece_fit_evidence(state: &LedgerState, piece_id: &str) -> PieceEvidence {
    use crate::model::{KeepDecision, OutcomeResult, ReasonAspect, ReferenceRole, TryOnOverall};
    // (date, ledger revision, signal, id): ordered so the latest report wins.
    let mut signals: Vec<(String, u64, Signal, String)> = Vec::new();
    if let Some(record) = state.record(piece_id)
        && let Some(reference) = state.piece(piece_id).and_then(|p| p.reference.as_ref())
    {
        let day: String = crate::service::iso(record.recorded_at)
            .chars()
            .take(10)
            .collect();
        let id = format!(
            "{piece_id}:reference_{}",
            crate::state::enum_text(&reference.role)
        );
        let signal = match reference.role {
            ReferenceRole::Baseline => Some(Signal::Positive),
            ReferenceRole::TooSmall | ReferenceRole::TooLarge => Some(Signal::Adverse),
            ReferenceRole::Partial => None,
        };
        if let Some(signal) = signal {
            signals.push((day, record.ledger_revision, signal, id));
        }
    }
    for event in state.piece_events(piece_id) {
        let body = &event.envelope.event;
        let fits = body.region_fit();
        let any_adverse = fits.iter().any(|f| is_adverse(f.verdict));
        let signal = match body {
            EventBody::TryOn(t) => {
                if any_adverse || t.overall == TryOnOverall::WrongSize {
                    Some(Signal::Adverse)
                } else if t.overall == TryOnOverall::Fits {
                    Some(Signal::Positive)
                } else {
                    None
                }
            }
            // Comfort- or style-only reports and all-unknown regions are not
            // fit evidence.
            EventBody::Wear(_) | EventBody::Feedback(_) if known_fits(fits).next().is_some() => {
                Some(if any_adverse {
                    Signal::Adverse
                } else {
                    Signal::Positive
                })
            }
            EventBody::Outcome(o) => match o.result {
                OutcomeResult::Missed => Some(Signal::Adverse),
                OutcomeResult::Matched if !any_adverse => Some(Signal::Positive),
                _ if any_adverse => Some(Signal::Adverse),
                _ => None,
            },
            EventBody::Decision(d)
                if d.decision == KeepDecision::Return
                    && d.reasons.iter().any(|r| r.aspect == ReasonAspect::Fit) =>
            {
                Some(Signal::Adverse)
            }
            EventBody::Wash(w)
                if w.observed_changes
                    .iter()
                    .any(|c| matches!(c.change, ChangeKind::Shrank | ChangeKind::Stretched)) =>
            {
                Some(Signal::Adverse)
            }
            _ => None,
        };
        if let Some(signal) = signal {
            signals.push((
                event.occurred_on.clone(),
                event.ledger_revision,
                signal,
                event.id.as_str().to_owned(),
            ));
        }
    }
    signals.sort_by(|a, b| (a.0.as_str(), a.1).cmp(&(b.0.as_str(), b.1)));
    let latest = signals.last().map(|s| s.2);
    let (positive, adverse): (Vec<_>, Vec<_>) =
        signals.into_iter().partition(|s| s.2 == Signal::Positive);
    let positive: Vec<String> = positive.into_iter().map(|s| s.3).collect();
    let adverse: Vec<String> = adverse.into_iter().map(|s| s.3).collect();
    let fit = match (positive.is_empty(), adverse.is_empty()) {
        (false, true) => FitEvidence::Proven,
        (true, false) => FitEvidence::Failed,
        (false, false) => FitEvidence::Mixed,
        (true, true) => FitEvidence::Untested,
    };
    PieceEvidence {
        piece_id: piece_id.to_owned(),
        fit,
        latest,
        positive,
        adverse,
    }
}

/// The direction of one fit report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Positive,
    Adverse,
}

impl PieceEvidence {
    /// The piece failed and nothing positive was reported after the failure:
    /// a later adverse report is not cancelled by an earlier success.
    pub fn failed_now(&self) -> bool {
        self.fit == FitEvidence::Failed
            || (self.fit == FitEvidence::Mixed && self.latest == Some(Signal::Adverse))
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
    /// Each material difference, e.g. `elastane 0% vs 5% (stretch content)`.
    pub material_differences: Vec<String>,
    pub fit: FitEvidence,
    /// The direction of the piece's latest fit report.
    pub latest: Option<Signal>,
    /// Failed, or mixed with an adverse latest report.
    pub failed_now: bool,
}

/// What the ledger supports for a candidate, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BasisAssessment {
    pub basis: Basis,
    pub reasons: Vec<String>,
    pub identity_matches: Vec<IdentityMatch>,
    /// Identity matches with the same version and material whose fit failed,
    /// or whose latest fit report is adverse after earlier positive ones.
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

/// Fibers that add stretch. Even a few percent changes how a garment fits and
/// recovers, so their presence and share must match exactly.
pub const STRETCH_FIBERS: [&str; 7] = [
    "elastane",
    "spandex",
    "lycra",
    "elastomultiester",
    "elasterell_p",
    "polyurethane",
    "rubber",
];

/// Largest difference, in percentage points, between two labels' share of a
/// stretch fiber that still counts as the same material.
pub const STRETCH_SHARE_TOLERANCE: f64 = 0.5;

/// The same for other fibers: care labels may round shares by a few points.
pub const FIBER_SHARE_TOLERANCE: f64 = 3.0;

fn share(fabric: &crate::model::Fabric, fiber: &str) -> f64 {
    fabric
        .composition
        .iter()
        .filter(|s| s.fiber.as_str() == fiber)
        .map(|s| s.percent)
        .sum()
}

/// How two variants' recorded materials compare, with each difference
/// spelled out. Material agrees only when both record a composition and:
///
/// - every fiber is present on both or neither;
/// - stretch fibers ([`STRETCH_FIBERS`]) differ by at most
///   [`STRETCH_SHARE_TOLERANCE`] points, other fibers by at most
///   [`FIBER_SHARE_TOLERANCE`];
/// - construction, recorded stretch (other than `unknown`) and pre-shrinking
///   agree wherever both sides record them.
pub fn material_comparison(a: &VariantRecord, b: &VariantRecord) -> (Agreement, Vec<String>) {
    let (Some(fa), Some(fb)) = (&a.fabric, &b.fabric) else {
        return (Agreement::Unrecorded, Vec::new());
    };
    if fa.composition.is_empty() || fb.composition.is_empty() {
        return (Agreement::Unrecorded, Vec::new());
    }
    let pct = omni_core::js::number_to_string;
    let mut differences = Vec::new();
    let fibers: std::collections::BTreeSet<&str> = fa
        .composition
        .iter()
        .chain(&fb.composition)
        .map(|s| s.fiber.as_str())
        .collect();
    for fiber in fibers {
        let (x, y) = (share(fa, fiber), share(fb, fiber));
        let stretch = STRETCH_FIBERS.contains(&fiber);
        let tolerance = if stretch {
            STRETCH_SHARE_TOLERANCE
        } else {
            FIBER_SHARE_TOLERANCE
        };
        if (x > 0.0) != (y > 0.0) || (x - y).abs() > tolerance {
            differences.push(format!(
                "{fiber} {}% vs {}%{}",
                pct(x),
                pct(y),
                if stretch { " (stretch content)" } else { "" }
            ));
        }
    }
    if let (Some(x), Some(y)) = (fa.construction, fb.construction)
        && x != y
    {
        differences.push(format!(
            "construction {} vs {}",
            crate::state::enum_text(&x),
            crate::state::enum_text(&y)
        ));
    }
    let recorded_stretch = |fabric: &crate::model::Fabric| {
        fabric
            .stretch
            .filter(|s| *s != crate::model::FabricStretch::Unknown)
    };
    if let (Some(x), Some(y)) = (recorded_stretch(fa), recorded_stretch(fb))
        && x != y
    {
        differences.push(format!(
            "stretch {} vs {}",
            crate::state::enum_text(&x),
            crate::state::enum_text(&y)
        ));
    }
    if let (Some(x), Some(y)) = (fa.pre_shrunk, fb.pre_shrunk)
        && x != y
    {
        differences.push(format!("pre-shrunk {x} vs {y}"));
    }
    let agreement = if differences.is_empty() {
        Agreement::Same
    } else {
        Agreement::Differs
    };
    (agreement, differences)
}

/// The strongest basis the ledger supports for `candidate`:
///
/// - `exact_repeat`: a prior piece of the same brand, model, cut, size label
///   and size system, with the same version (or none recorded on either) and
///   the same material ([`material_comparison`]), has proven fit, and no such
///   piece has adverse fit evidence (one whose latest report is adverse is a
///   known failure);
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
            let (material, material_differences) = material_comparison(variant, candidate);
            let found = IdentityMatch {
                piece_id: piece_id.to_owned(),
                variant_id: variant_id.to_owned(),
                version: version_agreement(variant.version.as_ref(), candidate.version.as_ref()),
                material,
                material_differences,
                fit: evidence.fit,
                latest: evidence.latest,
                failed_now: evidence.failed_now(),
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
                Agreement::Differs => reasons.push(format!(
                    "{piece_id}: the material differs ({})",
                    found.material_differences.join("; ")
                )),
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
                FitEvidence::Mixed if found.failed_now => reasons.push(format!(
                    "{piece_id} is the same variant and its latest fit report is adverse ({}), after earlier positive reports",
                    evidence.adverse.last().map_or("", String::as_str)
                )),
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
        .filter(|m| m.failed_now)
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

/// The region verdicts that say something about fit (not `unknown`). An
/// event with none of them is no fit evidence, whatever its comfort, style
/// or other ratings say.
pub fn known_fits(fits: &[RegionFit]) -> impl Iterator<Item = &RegionFit> {
    fits.iter().filter(|f| f.verdict != FitVerdict::Unknown)
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
