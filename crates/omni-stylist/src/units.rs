//! Length normalization and the measurement compatibility rules.
//!
//! Values are normalized to millimetres while the original value and unit
//! stay in the stored record. Two measurements are comparable only when they
//! describe the same dimension, with the same method, of the same subject
//! class (body against body, finished garment against finished garment). A
//! flat width is never doubled into a circumference and a body girth is never
//! compared with a garment measurement, because the ease between them is
//! unknown.

use serde::Serialize;

use crate::model::{
    LengthUnit, MeasureMethod, MeasurementRecord, StretchState, SubjectKind, WashState,
};

/// Millimetres per unit.
pub fn mm_per(unit: LengthUnit) -> f64 {
    match unit {
        LengthUnit::Mm => 1.0,
        LengthUnit::Cm => 10.0,
        LengthUnit::In => 25.4,
    }
}

/// `value` in millimetres, rounded to 0.1 mm.
pub fn to_mm(value: f64, unit: LengthUnit) -> f64 {
    round1(value * mm_per(unit))
}

pub fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// The largest plausible length: 3 m.
pub const MAX_MM: f64 = 3000.0;

/// Body or finished garment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectClass {
    Body,
    Garment,
}

pub fn subject_class(kind: SubjectKind) -> SubjectClass {
    match kind {
        SubjectKind::Body => SubjectClass::Body,
        SubjectKind::Piece | SubjectKind::VariantPublished => SubjectClass::Garment,
    }
}

/// Why two measurements cannot be compared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Incompatibility {
    DifferentDimension,
    BodyVersusGarment,
    DifferentMethod,
}

impl Incompatibility {
    pub fn explain(&self) -> &'static str {
        match self {
            Incompatibility::DifferentDimension => "different dimensions are not comparable",
            Incompatibility::BodyVersusGarment => {
                "a body measurement and a finished-garment measurement are not directly comparable: the intended ease is unknown"
            }
            Incompatibility::DifferentMethod => {
                "different methods (for example flat width against circumference) are kept separate: a flat width is not doubled automatically; re-measure with the same method"
            }
        }
    }
}

/// A caveat on a comparable pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Caveat {
    EndpointsDiffer,
    WashStateDiffers,
    WashStateUnknown,
    StretchStateDiffers,
    StretchStateUnknown,
    PublishedChart,
}

impl Caveat {
    pub fn explain(&self) -> &'static str {
        match self {
            Caveat::EndpointsDiffer => {
                "the recorded endpoints differ; confirm both used the same landmarks"
            }
            Caveat::WashStateDiffers => {
                "one garment was washed and the other was not; shrinkage may explain part of the difference"
            }
            Caveat::WashStateUnknown => "wash state is unknown for at least one garment",
            Caveat::StretchStateDiffers => "the garments were measured under different tension",
            Caveat::StretchStateUnknown => {
                "tension while measuring is unknown for at least one garment"
            }
            Caveat::PublishedChart => {
                "a published chart is a brand claim with its own protocol and tolerance, not a measurement of this garment"
            }
        }
    }
}

/// The comparison of two measurements.
#[derive(Clone, Debug, PartialEq)]
pub enum Comparison {
    Comparable {
        /// `candidate - reference` in millimetres.
        delta_mm: f64,
        /// The summed tolerance, when both are known.
        tolerance_mm: Option<f64>,
        caveats: Vec<Caveat>,
    },
    Incompatible(Incompatibility),
}

/// Compares `candidate` with `reference`.
pub fn compare(reference: &MeasurementRecord, candidate: &MeasurementRecord) -> Comparison {
    if reference.dimension != candidate.dimension {
        return Comparison::Incompatible(Incompatibility::DifferentDimension);
    }
    if subject_class(reference.subject.kind) != subject_class(candidate.subject.kind) {
        return Comparison::Incompatible(Incompatibility::BodyVersusGarment);
    }
    if reference.method != candidate.method {
        return Comparison::Incompatible(Incompatibility::DifferentMethod);
    }
    let mut caveats = Vec::new();
    if normalize_endpoints(&reference.endpoints) != normalize_endpoints(&candidate.endpoints) {
        caveats.push(Caveat::EndpointsDiffer);
    }
    if let (Some(a), Some(b)) = (&reference.garment_state, &candidate.garment_state) {
        if a.wash == WashState::Unknown || b.wash == WashState::Unknown {
            caveats.push(Caveat::WashStateUnknown);
        } else if a.wash != b.wash {
            caveats.push(Caveat::WashStateDiffers);
        }
        if a.stretch == StretchState::Unknown || b.stretch == StretchState::Unknown {
            caveats.push(Caveat::StretchStateUnknown);
        } else if a.stretch != b.stretch {
            caveats.push(Caveat::StretchStateDiffers);
        }
    }
    if reference.subject.kind == SubjectKind::VariantPublished
        || candidate.subject.kind == SubjectKind::VariantPublished
    {
        caveats.push(Caveat::PublishedChart);
    }
    let tolerance_mm = match (reference.tolerance, candidate.tolerance) {
        (Some(a), Some(b)) => Some(round1(to_mm(a, reference.unit) + to_mm(b, candidate.unit))),
        _ => None,
    };
    Comparison::Comparable {
        delta_mm: round1(
            to_mm(candidate.value, candidate.unit) - to_mm(reference.value, reference.unit),
        ),
        tolerance_mm,
        caveats,
    }
}

fn normalize_endpoints(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The disagreement threshold for repeated measurements of the same thing:
/// the summed tolerances when both are stated, otherwise 15 mm for the body
/// (self-measurement with a soft tape varies by about that much) and 10 mm
/// for garments. This only raises a flag for review; it decides nothing.
pub fn disagreement_threshold_mm(a: &MeasurementRecord, b: &MeasurementRecord) -> f64 {
    match (a.tolerance, b.tolerance) {
        (Some(ta), Some(tb)) => to_mm(ta, a.unit) + to_mm(tb, b.unit),
        _ => match subject_class(a.subject.kind) {
            SubjectClass::Body => 15.0,
            SubjectClass::Garment => 10.0,
        },
    }
}

/// Methods that make sense for a subject class.
pub fn method_allowed(class: SubjectClass, method: MeasureMethod) -> bool {
    !(class == SubjectClass::Body && method == MeasureMethod::FlatWidth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Certainty, GarmentState, MeasurementSubject, Provenance, Source};
    use crate::types::Slug;

    fn measurement(
        kind: SubjectKind,
        dimension: &str,
        method: MeasureMethod,
        value: f64,
        unit: LengthUnit,
    ) -> MeasurementRecord {
        MeasurementRecord {
            subject: MeasurementSubject { kind, id: None },
            dimension: Slug(dimension.to_owned()),
            method,
            endpoints: "armpit to armpit".to_owned(),
            value,
            unit,
            tolerance: None,
            garment_state: (kind != SubjectKind::Body).then_some(GarmentState {
                stretch: StretchState::Relaxed,
                wash: WashState::Washed,
                wash_count: None,
                after_event_id: None,
            }),
            body_state: None,
            observed_on: "2026-10-01".to_owned(),
            provenance: Provenance {
                certainty: Certainty::Explicit,
                source: Source::UserMeasurement,
                observed_on: None,
                evidence_ids: Vec::new(),
                note: None,
            },
        }
    }

    #[test]
    fn units_normalize_to_millimetres() {
        assert_eq!(to_mm(21.0, LengthUnit::In), 533.4);
        assert_eq!(to_mm(53.5, LengthUnit::Cm), 535.0);
        assert_eq!(to_mm(535.0, LengthUnit::Mm), 535.0);
    }

    #[test]
    fn same_method_garments_compare_across_units() {
        let reference = measurement(
            SubjectKind::Piece,
            "chest",
            MeasureMethod::FlatWidth,
            21.0,
            LengthUnit::In,
        );
        let candidate = measurement(
            SubjectKind::VariantPublished,
            "chest",
            MeasureMethod::FlatWidth,
            54.0,
            LengthUnit::Cm,
        );
        let Comparison::Comparable {
            delta_mm, caveats, ..
        } = compare(&reference, &candidate)
        else {
            panic!("expected comparable");
        };
        assert_eq!(delta_mm, 6.6);
        assert_eq!(caveats, vec![Caveat::PublishedChart]);
    }

    #[test]
    fn flat_width_and_circumference_stay_separate() {
        let flat = measurement(
            SubjectKind::Piece,
            "chest",
            MeasureMethod::FlatWidth,
            53.0,
            LengthUnit::Cm,
        );
        let round = measurement(
            SubjectKind::Piece,
            "chest",
            MeasureMethod::Circumference,
            106.0,
            LengthUnit::Cm,
        );
        assert_eq!(
            compare(&flat, &round),
            Comparison::Incompatible(Incompatibility::DifferentMethod)
        );
    }

    #[test]
    fn body_and_garment_are_not_compared() {
        let body = measurement(
            SubjectKind::Body,
            "chest",
            MeasureMethod::Circumference,
            100.0,
            LengthUnit::Cm,
        );
        let garment = measurement(
            SubjectKind::Piece,
            "chest",
            MeasureMethod::Circumference,
            106.0,
            LengthUnit::Cm,
        );
        assert_eq!(
            compare(&body, &garment),
            Comparison::Incompatible(Incompatibility::BodyVersusGarment)
        );
        assert!(!method_allowed(
            SubjectClass::Body,
            MeasureMethod::FlatWidth
        ));
    }

    #[test]
    fn wash_state_differences_are_caveats() {
        let washed = measurement(
            SubjectKind::Piece,
            "body_length",
            MeasureMethod::Linear,
            70.0,
            LengthUnit::Cm,
        );
        let mut unwashed = washed.clone();
        if let Some(state) = unwashed.garment_state.as_mut() {
            state.wash = WashState::Unwashed;
        }
        let Comparison::Comparable { caveats, .. } = compare(&unwashed, &washed) else {
            panic!("expected comparable");
        };
        assert_eq!(caveats, vec![Caveat::WashStateDiffers]);
    }
}
