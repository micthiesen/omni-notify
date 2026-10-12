//! The typed ledger contents: records (revisioned or immutable) and events
//! (immutable). These types are both the MCP input contract and the stored
//! payload: the derived JSON schema validates every write at the MCP boundary,
//! and [`crate::validate`] checks cross-record rules against the ledger.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{Category, Currency, EventId, EvidenceId, LedgerId, RecordId, Slug};

/// How a fact is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    /// Stated by the owner or directly measured or observed.
    Explicit,
    /// Derived by the assistant from other evidence.
    Inferred,
    /// Recorded as an open question.
    Unknown,
}

/// Where a fact came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    UserStatement,
    UserMeasurement,
    GuidedMeasurement,
    Receipt,
    Label,
    Retailer,
    Brand,
    Review,
    Photo,
    AssistantInference,
    Import,
    Other,
}

/// Provenance carried by every record and event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Provenance {
    pub certainty: Certainty,
    pub source: Source,
    /// When the fact was true or observed (YYYY-MM-DD), if known.
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub observed_on: Option<String>,
    /// Supporting evidence from `stylist_evidence_store`.
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub evidence_ids: Vec<EvidenceId>,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

/// An amount of money.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Money {
    #[schemars(range(min = 0.0, max = 1_000_000.0))]
    pub amount: f64,
    pub currency: Currency,
}

/// Length units; values are normalized to millimetres and the original kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    Mm,
    Cm,
    In,
}

// ---------------------------------------------------------------------------
// Records

/// A record body. `measurement`, `profile_snapshot` and `prediction` are
/// immutable (correct them with a new record that `supersedes` the old one);
/// the other kinds are revised with `expectedRevision`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordBody {
    /// A product variant: one brand, model, cut, colour and size as sold.
    Variant(VariantRecord),
    /// One individual garment the owner has or had, which can change through
    /// washing and alteration.
    Piece(PieceRecord),
    /// One measurement of the body, an owned piece, or a variant's published chart.
    Measurement(MeasurementRecord),
    /// A dated snapshot of body attributes and fit sensitivities.
    ProfileSnapshot(ProfileSnapshotRecord),
    /// A category- or article-specific preference with a validity date.
    Preference(PreferenceRecord),
    /// A frozen recommendation forecast, saved before any outcome is known.
    Prediction(PredictionRecord),
    /// A narrow, evidence-linked learning rule.
    Rule(RuleRecord),
    /// A genuine wardrobe gap, replacement or restock.
    Need(NeedRecord),
}

/// The kinds of record.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Variant,
    Piece,
    Measurement,
    ProfileSnapshot,
    Preference,
    Prediction,
    Rule,
    Need,
}

impl RecordKind {
    pub const ALL: [RecordKind; 8] = [
        RecordKind::Variant,
        RecordKind::Piece,
        RecordKind::Measurement,
        RecordKind::ProfileSnapshot,
        RecordKind::Preference,
        RecordKind::Prediction,
        RecordKind::Rule,
        RecordKind::Need,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RecordKind::Variant => "variant",
            RecordKind::Piece => "piece",
            RecordKind::Measurement => "measurement",
            RecordKind::ProfileSnapshot => "profile_snapshot",
            RecordKind::Preference => "preference",
            RecordKind::Prediction => "prediction",
            RecordKind::Rule => "rule",
            RecordKind::Need => "need",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// The id prefix of this kind.
    pub fn prefix(self) -> &'static str {
        match self {
            RecordKind::Variant => "var",
            RecordKind::Piece => "pc",
            RecordKind::Measurement => "ms",
            RecordKind::ProfileSnapshot => "pf",
            RecordKind::Preference => "pref",
            RecordKind::Prediction => "pred",
            RecordKind::Rule => "rule",
            RecordKind::Need => "need",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.prefix() == prefix)
    }

    /// Immutable kinds are corrected by a superseding record, never revised.
    pub fn is_immutable(self) -> bool {
        matches!(
            self,
            RecordKind::Measurement | RecordKind::ProfileSnapshot | RecordKind::Prediction
        )
    }
}

impl RecordBody {
    pub fn kind(&self) -> RecordKind {
        match self {
            RecordBody::Variant(_) => RecordKind::Variant,
            RecordBody::Piece(_) => RecordKind::Piece,
            RecordBody::Measurement(_) => RecordKind::Measurement,
            RecordBody::ProfileSnapshot(_) => RecordKind::ProfileSnapshot,
            RecordBody::Preference(_) => RecordKind::Preference,
            RecordBody::Prediction(_) => RecordKind::Prediction,
            RecordBody::Rule(_) => RecordKind::Rule,
            RecordBody::Need(_) => RecordKind::Need,
        }
    }
}

/// Sizing systems.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SizeSystem {
    /// XS, S, M, L, XL, ...
    Alpha,
    /// Numeric chest or jacket sizes such as 38 or 40R.
    UsNumeric,
    Eu,
    Uk,
    It,
    Jp,
    /// Waist by inseam, such as 32x32.
    WaistInseam,
    /// Collar by sleeve, such as 15.5/34.
    NeckSleeve,
    ShoeUs,
    ShoeEu,
    ShoeUk,
    OneSize,
    Other,
}

/// Knit, woven, or another construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Construction {
    Knit,
    Woven,
    NonWoven,
    Leather,
    Other,
}

/// How a fabric stretches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FabricStretch {
    None,
    Mechanical,
    Elastane,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct FiberShare {
    /// `cotton`, `wool`, `elastane`, ...
    pub fiber: Slug,
    #[schemars(range(min = 0.0, max = 100.0))]
    pub percent: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Fabric {
    #[serde(default)]
    #[schemars(length(max = 10))]
    pub composition: Vec<FiberShare>,
    pub construction: Option<Construction>,
    #[schemars(range(min = 0.0, max = 2000.0))]
    pub weight_gsm: Option<f64>,
    pub stretch: Option<FabricStretch>,
    /// Sold as pre-shrunk, sanforized or garment-washed.
    pub pre_shrunk: Option<bool>,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

/// A retailer listing. URLs and listing text are untrusted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RetailerRef {
    #[schemars(length(min = 1, max = 200))]
    pub retailer: String,
    #[schemars(length(max = 2000))]
    pub url: Option<String>,
    #[schemars(length(max = 200))]
    pub product_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct VariantRecord {
    #[schemars(length(min = 1, max = 120))]
    pub brand: String,
    /// The product line or model name, e.g. `Supima Cotton Crew Neck T-Shirt`.
    #[schemars(length(min = 1, max = 200))]
    pub model: String,
    pub category: Category,
    /// The size as labelled, e.g. `M`, `32x32`, `40R`.
    #[schemars(length(min = 1, max = 40))]
    pub size_label: String,
    pub size_system: SizeSystem,
    #[schemars(length(max = 120))]
    pub sku: Option<String>,
    #[schemars(length(max = 120))]
    pub style_code: Option<String>,
    /// Named cut or fit, e.g. `slim`, `regular`, `relaxed taper`.
    #[schemars(length(max = 120))]
    pub cut: Option<String>,
    #[schemars(length(max = 120))]
    pub colour: Option<String>,
    /// Season, year or pattern revision; a change can mean a new pattern.
    #[schemars(length(max = 120))]
    pub version: Option<String>,
    pub fabric: Option<Fabric>,
    /// Care label text.
    #[schemars(length(max = 1000))]
    pub care: Option<String>,
    #[serde(default)]
    #[schemars(length(max = 10))]
    pub retailer_refs: Vec<RetailerRef>,
    #[schemars(length(max = 4000))]
    pub notes: Option<String>,
    pub provenance: Provenance,
    /// Save even though an identical brand/model/size/cut/colour/version exists.
    pub allow_duplicate: Option<bool>,
}

/// A piece's standing as a sizing reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceRole {
    /// Fits well in the listed regions: the strongest baseline.
    Baseline,
    /// Known to be too small in the listed regions.
    TooSmall,
    /// Known to be too large in the listed regions.
    TooLarge,
    /// Fits in some regions only.
    Partial,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Reference {
    /// Categories this piece is a reference for (`tops/tshirt`, `tops`).
    #[schemars(length(min = 1, max = 10))]
    pub categories: Vec<Category>,
    pub role: ReferenceRole,
    /// Regions the role applies to; empty means the whole garment.
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub regions: Vec<Slug>,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

/// A piece's status at creation; later changes come only from lifecycle
/// events (`acquired`, `decision`, `returned`, `retired`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InitialStatus {
    Owned,
    Undecided,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct PieceRecord {
    /// A short name the owner recognises, e.g. `navy Supima tee (2024)`.
    #[schemars(length(min = 1, max = 200))]
    pub label: String,
    pub category: Category,
    /// The exact variant, when known.
    pub variant_id: Option<RecordId>,
    /// What the piece is when no variant is recorded.
    #[schemars(length(max = 1000))]
    pub description: Option<String>,
    /// Status at creation (default `owned`). It never changes: omit it or
    /// repeat it on revisions.
    pub initial_status: Option<InitialStatus>,
    pub reference: Option<Reference>,
    #[schemars(length(max = 4000))]
    pub notes: Option<String>,
    pub provenance: Provenance,
}

/// What a measurement describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// The owner's body (no id).
    Body,
    /// One owned piece, as finished garment.
    Piece,
    /// A variant's published finished-garment chart for its size.
    VariantPublished,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct MeasurementSubject {
    #[serde(rename = "type")]
    pub kind: SubjectKind,
    /// The piece or variant id; omit for `body`.
    pub id: Option<RecordId>,
}

/// How a length was taken. A flat width is half a garment's circumference
/// at best; it is never converted automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeasureMethod {
    /// A girth around the body or a garment.
    Circumference,
    /// Straight across a garment laid flat (pit to pit, waist across).
    FlatWidth,
    /// A straight length (sleeve, inseam, body length, shoulder seam to seam).
    Linear,
    /// Along a curve, such as a rise or crotch length.
    Curve,
}

/// Whether the garment was stretched while measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StretchState {
    /// Laid flat without tension.
    Relaxed,
    /// Pulled smooth and taut, not stretched.
    Taut,
    Stretched,
    Unknown,
}

/// Whether the garment had been washed when measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WashState {
    Unwashed,
    Washed,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct GarmentState {
    pub stretch: StretchState,
    pub wash: WashState,
    /// Washes before this measurement, when known.
    #[schemars(range(max = 1000))]
    pub wash_count: Option<u32>,
    /// The `wash` event this measurement follows, when known.
    pub after_event_id: Option<EventId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct MeasurementRecord {
    pub subject: MeasurementSubject,
    /// `chest`, `waist`, `hip`, `shoulder_width`, `sleeve`, `body_length`,
    /// `inseam`, `rise_front`, `thigh`, `knee`, `leg_opening`, `neck`, ...
    pub dimension: Slug,
    pub method: MeasureMethod,
    /// The exact endpoints and protocol, e.g. `armpit seam to armpit seam,
    /// 2.5 cm below the armhole, garment buttoned`.
    #[schemars(length(min = 3, max = 500))]
    pub endpoints: String,
    /// The value as taken, in `unit`.
    #[schemars(range(min = 0.0, max = 10000.0))]
    pub value: f64,
    pub unit: LengthUnit,
    /// Plus or minus, in `unit`, when known.
    #[schemars(range(min = 0.0, max = 1000.0))]
    pub tolerance: Option<f64>,
    /// Required for pieces and published charts; not allowed for the body.
    pub garment_state: Option<GarmentState>,
    /// Body posture or layering, e.g. `relaxed exhale, over a thin tee`.
    #[schemars(length(max = 500))]
    pub body_state: Option<String>,
    /// When the measurement was taken (YYYY-MM-DD).
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub observed_on: String,
    pub provenance: Provenance,
}

/// Units for profile attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileUnit {
    Mm,
    Cm,
    In,
    Kg,
    Lb,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ProfileAttribute {
    /// `height`, `weight`, `build`, `shoulder_slope`, `posture`, `arm_length`, ...
    pub name: Slug,
    #[schemars(range(min = 0.0, max = 100000.0))]
    pub number: Option<f64>,
    pub unit: Option<ProfileUnit>,
    #[schemars(length(max = 500))]
    pub text: Option<String>,
    pub certainty: Certainty,
    pub source: Source,
    #[schemars(length(max = 500))]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct FitSensitivity {
    pub region: Slug,
    #[schemars(length(min = 1, max = 500))]
    pub note: String,
    pub certainty: Certainty,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ProfileSnapshotRecord {
    /// When these attributes were true (YYYY-MM-DD).
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub observed_on: String,
    #[serde(default)]
    #[schemars(length(max = 50))]
    pub attributes: Vec<ProfileAttribute>,
    #[serde(default)]
    #[schemars(length(max = 30))]
    pub fit_sensitivities: Vec<FitSensitivity>,
    #[schemars(length(max = 4000))]
    pub notes: Option<String>,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
    Silhouette,
    Length,
    Roominess,
    Layering,
    Tactile,
    Care,
    Style,
    Colour,
    Fabric,
    Quality,
    Value,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Require,
    Prefer,
    Avoid,
    Reject,
}

impl Strength {
    pub fn is_positive(self) -> bool {
        matches!(self, Strength::Require | Strength::Prefer)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct PreferenceRecord {
    /// A category path, or `any` for every category.
    pub category: Category,
    pub aspect: Aspect,
    pub region: Option<Slug>,
    #[schemars(length(min = 1, max = 1000))]
    pub statement: String,
    pub strength: Strength,
    /// From when this preference holds (YYYY-MM-DD). Revise the record with a
    /// later date when taste changes; earlier revisions stay retrievable.
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub valid_from: String,
    pub provenance: Provenance,
}

/// Expected or reported fit of one region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FitVerdict {
    TooTight,
    SlightlyTight,
    Good,
    SlightlyLoose,
    TooLoose,
    TooShort,
    SlightlyShort,
    SlightlyLong,
    TooLong,
    Unknown,
}

/// How far the evidence behind a forecast is from the candidate.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// No piece of this brand has recorded fit evidence.
    NewBrandInference,
    /// Some piece of the brand has recorded fit evidence, but no proven exact repeat.
    KnownBrandNewCut,
    /// A piece of the same brand, model, cut, size, version and fiber
    /// composition has proven fit, and none of them failed.
    ExactRepeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PredictionDecision {
    Recommend,
    RecommendWithCaution,
    /// Too much crucial uncertainty or risk to recommend a size.
    Abstain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RegionExpectation {
    pub region: Slug,
    pub expected: FitVerdict,
    #[schemars(length(min = 1, max = 1000))]
    pub rationale: String,
    /// Measurements, pieces, events or rules behind the expectation.
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub evidence_refs: Vec<LedgerId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct FailureMode {
    pub region: Option<Slug>,
    #[schemars(length(min = 1, max = 1000))]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ReturnRisk {
    pub level: RiskLevel,
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub return_by: Option<String>,
    #[schemars(length(max = 500))]
    pub cost: Option<String>,
    #[schemars(length(max = 500))]
    pub effort: Option<String>,
    #[schemars(length(min = 1, max = 1000))]
    pub rationale: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct SizeAlternative {
    #[schemars(length(min = 1, max = 40))]
    pub size_label: String,
    #[schemars(length(min = 1, max = 1000))]
    pub why_not: String,
}

/// Facts verified at shopping time; listings are untrusted and change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ShoppingCheck {
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub checked_on: String,
    #[schemars(length(min = 1, max = 200))]
    pub retailer: String,
    pub in_stock: Option<bool>,
    pub delivered_price: Option<Money>,
    #[schemars(length(max = 1000))]
    pub return_policy: Option<String>,
    #[schemars(length(max = 2000))]
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct PredictionRecord {
    /// The exact candidate variant, including its size.
    pub variant_id: RecordId,
    /// Must equal the variant's `sizeLabel`.
    #[schemars(length(min = 1, max = 40))]
    pub recommended_size: String,
    #[serde(default)]
    #[schemars(length(max = 6))]
    pub alternatives: Vec<SizeAlternative>,
    pub decision: PredictionDecision,
    /// Checked against the ledger: a claim stronger than the owned history supports is rejected.
    pub basis: Basis,
    /// Expected fit per region; at least one unless abstaining.
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub region_expectations: Vec<RegionExpectation>,
    pub likely_failure_mode: FailureMode,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub evidence_gaps: Vec<String>,
    /// Reference pieces and measurements used.
    #[serde(default)]
    #[schemars(length(max = 30))]
    pub references_used: Vec<RecordId>,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub rules_applied: Vec<RecordId>,
    pub return_risk: ReturnRisk,
    pub shopping_check: Option<ShoppingCheck>,
    #[schemars(length(max = 2000))]
    pub style_notes: Option<String>,
    #[schemars(length(max = 2000))]
    pub value_notes: Option<String>,
    pub need_id: Option<RecordId>,
    /// The `snapshot.ledgerRevision` of the `stylist_context_get` result this
    /// forecast was made from.
    #[schemars(range(min = 0))]
    pub context_revision: u64,
    /// Required for new forecasts: that result's `snapshot.contextToken`,
    /// unchanged. Omni re-renders the context it names and rejects the
    /// forecast with [context_mismatch] unless it matches. (Optional only so
    /// forecasts frozen before tokens existed still decode.)
    pub context_token: Option<crate::types::ContextToken>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    Proposed,
    /// Requires a regression entry for every in-scope case and none inconsistent.
    Active,
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    RunsSmall,
    RunsLarge,
    RunsShort,
    RunsLong,
    Shrinks,
    Stretches,
    Other,
}

/// What a rule covers. At least one of brand, model or category is required.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RuleScope {
    #[schemars(length(min = 1, max = 120))]
    pub brand: Option<String>,
    #[schemars(length(min = 1, max = 200))]
    pub model: Option<String>,
    /// Matches this category and those below it.
    pub category: Option<Category>,
    #[schemars(length(min = 1, max = 40))]
    pub size_label: Option<String>,
    pub size_system: Option<SizeSystem>,
    /// Limits fit cases to reports about this region.
    pub region: Option<Slug>,
    /// Limits to variants whose composition includes this fiber.
    pub fiber: Option<Slug>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RuleAdjustment {
    pub region: Option<Slug>,
    pub direction: Direction,
    #[schemars(range(min = 0.0, max = 1000.0))]
    pub magnitude: Option<f64>,
    pub unit: Option<LengthUnit>,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegressionVerdict {
    Consistent,
    Inconsistent,
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RegressionCase {
    /// A fit case (event id) the rule was checked against.
    pub case_id: EventId,
    pub verdict: RegressionVerdict,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RuleRecord {
    #[schemars(length(min = 1, max = 1000))]
    pub statement: String,
    pub scope: RuleScope,
    pub adjustment: RuleAdjustment,
    pub basis: Basis,
    pub status: RuleStatus,
    /// The events, predictions or measurements that motivate the rule.
    #[schemars(length(min = 1, max = 30))]
    pub evidence: Vec<LedgerId>,
    /// One entry per in-scope fit case; `stylist_context_get` lists cases a
    /// rule has not been checked against.
    #[serde(default)]
    #[schemars(length(max = 200))]
    pub regression: Vec<RegressionCase>,
    pub supersedes_rule_id: Option<RecordId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NeedReason {
    Gap,
    Replacement,
    Restock,
    Upgrade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NeedStatus {
    Open,
    Paused,
    Satisfied,
    Dropped,
}

/// Shopping constraints. A budget is a preference, never permission to buy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct NeedConstraints {
    pub budget: Option<Money>,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub must_have: Vec<String>,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub avoid: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct NeedRecord {
    pub category: Category,
    pub reason: NeedReason,
    #[schemars(length(min = 1, max = 1000))]
    pub description: String,
    pub priority: Priority,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub related_piece_ids: Vec<RecordId>,
    pub constraints: Option<NeedConstraints>,
    pub status: NeedStatus,
    /// Revisit on or after this date (YYYY-MM-DD).
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub review_after: Option<String>,
    pub provenance: Provenance,
}

// ---------------------------------------------------------------------------
// Events

/// One region's fit as reported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RegionFit {
    pub region: Slug,
    pub verdict: FitVerdict,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    Positive,
    Mixed,
    Negative,
}

/// A rating with an optional note.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Assessment {
    pub rating: Rating,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct OrderInfo {
    #[schemars(length(min = 1, max = 200))]
    pub retailer: String,
    #[schemars(length(max = 200))]
    pub order_ref: Option<String>,
    /// The line within the order, when an order has several items.
    #[schemars(length(max = 200))]
    pub line_ref: Option<String>,
    pub price: Option<Money>,
    /// Price including shipping and tax, as charged.
    pub delivered_total: Option<Money>,
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub return_by: Option<String>,
    #[schemars(length(max = 1000))]
    pub return_policy: Option<String>,
    pub final_sale: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TryOnOverall {
    Fits,
    Close,
    WrongSize,
    Unresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Water {
    Cold,
    Warm,
    Hot,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Drying {
    LineDry,
    FlatDry,
    TumbleLow,
    TumbleMedium,
    TumbleHigh,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Shrank,
    Stretched,
    Pilled,
    Faded,
    Twisted,
    LostShape,
    NoChange,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct WashChange {
    pub region: Slug,
    pub change: ChangeKind,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeepDecision {
    Keep,
    Return,
    Undecided,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasonAspect {
    Fit,
    Style,
    Comfort,
    Quality,
    Value,
    Need,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DecisionReason {
    pub aspect: ReasonAspect,
    pub region: Option<Slug>,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetireReason {
    WornOut,
    NoLongerFits,
    StyleChange,
    Damaged,
    Donated,
    Sold,
    Lost,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackTiming {
    Immediate,
    AfterWear,
    AfterWash,
    /// Weeks or months later, including delayed regret.
    Delayed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeResult {
    Matched,
    Partial,
    Missed,
    /// Not tried, not worn, or too early to tell.
    Unresolved,
    NotPurchased,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionStatus {
    Open,
    Resolved,
}

/// An event body. Every event is immutable; correct one by appending an event
/// of the same type with `supersedesEventId`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventBody {
    /// A piece arrived. One per retailer order line; duplicates are rejected.
    Acquired(AcquiredEvent),
    /// A try-on, before any decision.
    TryOn(TryOnEvent),
    /// A day of wear.
    Wear(WearEvent),
    /// A wash, with any observed change. Re-measure afterwards when possible.
    Wash(WashEvent),
    Alteration(AlterationEvent),
    /// Keep, return or still undecided, with reasons. Keeping is not fit success.
    Decision(DecisionEvent),
    Returned(ReturnedEvent),
    Retired(RetiredEvent),
    /// Fit, style, comfort, quality, value and regret, kept separate.
    Feedback(FeedbackEvent),
    /// The result of a frozen prediction. The prediction itself never changes.
    Outcome(OutcomeEvent),
    /// Flags or resolves conflicting evidence.
    Contradiction(ContradictionEvent),
    Note(NoteEvent),
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Acquired,
    TryOn,
    Wear,
    Wash,
    Alteration,
    Decision,
    Returned,
    Retired,
    Feedback,
    Outcome,
    Contradiction,
    Note,
}

impl EventType {
    pub const ALL: [EventType; 12] = [
        EventType::Acquired,
        EventType::TryOn,
        EventType::Wear,
        EventType::Wash,
        EventType::Alteration,
        EventType::Decision,
        EventType::Returned,
        EventType::Retired,
        EventType::Feedback,
        EventType::Outcome,
        EventType::Contradiction,
        EventType::Note,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventType::Acquired => "acquired",
            EventType::TryOn => "try_on",
            EventType::Wear => "wear",
            EventType::Wash => "wash",
            EventType::Alteration => "alteration",
            EventType::Decision => "decision",
            EventType::Returned => "returned",
            EventType::Retired => "retired",
            EventType::Feedback => "feedback",
            EventType::Outcome => "outcome",
            EventType::Contradiction => "contradiction",
            EventType::Note => "note",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == value)
    }
}

impl EventBody {
    pub fn event_type(&self) -> EventType {
        match self {
            EventBody::Acquired(_) => EventType::Acquired,
            EventBody::TryOn(_) => EventType::TryOn,
            EventBody::Wear(_) => EventType::Wear,
            EventBody::Wash(_) => EventType::Wash,
            EventBody::Alteration(_) => EventType::Alteration,
            EventBody::Decision(_) => EventType::Decision,
            EventBody::Returned(_) => EventType::Returned,
            EventBody::Retired(_) => EventType::Retired,
            EventBody::Feedback(_) => EventType::Feedback,
            EventBody::Outcome(_) => EventType::Outcome,
            EventBody::Contradiction(_) => EventType::Contradiction,
            EventBody::Note(_) => EventType::Note,
        }
    }

    /// The piece this event is about, when it has one.
    pub fn piece_id(&self) -> Option<&RecordId> {
        match self {
            EventBody::Acquired(e) => Some(&e.piece_id),
            EventBody::TryOn(e) => e.piece_id.as_ref(),
            EventBody::Wear(e) => Some(&e.piece_id),
            EventBody::Wash(e) => Some(&e.piece_id),
            EventBody::Alteration(e) => Some(&e.piece_id),
            EventBody::Decision(e) => Some(&e.piece_id),
            EventBody::Returned(e) => Some(&e.piece_id),
            EventBody::Retired(e) => Some(&e.piece_id),
            EventBody::Feedback(e) => e
                .target_id
                .as_str()
                .starts_with("pc_")
                .then_some(&e.target_id),
            EventBody::Outcome(e) => e.piece_id.as_ref(),
            EventBody::Contradiction(_) | EventBody::Note(_) => None,
        }
    }

    /// The prediction this event is about, when it has one.
    pub fn prediction_id(&self) -> Option<&RecordId> {
        match self {
            EventBody::Acquired(e) => e.prediction_id.as_ref(),
            EventBody::TryOn(e) => e.prediction_id.as_ref(),
            EventBody::Feedback(e) => e
                .target_id
                .as_str()
                .starts_with("pred_")
                .then_some(&e.target_id),
            EventBody::Outcome(e) => Some(&e.prediction_id),
            _ => None,
        }
    }

    /// Region fit reports carried by the event.
    pub fn region_fit(&self) -> &[RegionFit] {
        match self {
            EventBody::TryOn(e) => &e.region_fit,
            EventBody::Wear(e) => &e.region_fit,
            EventBody::Feedback(e) => &e.region_fit,
            EventBody::Outcome(e) => &e.region_results,
            _ => &[],
        }
    }

    /// Whether this is a fit case that rules are regression-checked against.
    pub fn is_fit_case(&self) -> bool {
        match self {
            EventBody::TryOn(_) | EventBody::Outcome(_) => true,
            EventBody::Wear(e) => !e.region_fit.is_empty(),
            EventBody::Feedback(e) => !e.region_fit.is_empty(),
            EventBody::Wash(e) => e
                .observed_changes
                .iter()
                .any(|c| c.change != ChangeKind::NoChange),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcquiredStatus {
    /// Still within a try-on or return window.
    Undecided,
    Owned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct AcquiredEvent {
    pub piece_id: RecordId,
    pub order: Option<OrderInfo>,
    /// The frozen prediction this purchase followed, if any.
    pub prediction_id: Option<RecordId>,
    pub status_after: AcquiredStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct TryOnEvent {
    pub piece_id: Option<RecordId>,
    pub prediction_id: Option<RecordId>,
    #[schemars(length(min = 1, max = 20))]
    pub region_fit: Vec<RegionFit>,
    pub overall: TryOnOverall,
    pub comfort: Option<Assessment>,
    pub style: Option<Assessment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct WearEvent {
    pub piece_id: RecordId,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub region_fit: Vec<RegionFit>,
    pub comfort: Option<Assessment>,
    /// Occasion or conditions, e.g. `office, 8 hours, warm`.
    #[schemars(length(max = 500))]
    pub context: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct WashEvent {
    pub piece_id: RecordId,
    pub water: Water,
    pub drying: Drying,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub observed_changes: Vec<WashChange>,
    /// Measurements taken after this wash.
    #[serde(default)]
    #[schemars(length(max = 30))]
    pub measurement_ids: Vec<RecordId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct AlterationEvent {
    pub piece_id: RecordId,
    #[schemars(length(min = 1, max = 1000))]
    pub description: String,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub regions: Vec<Slug>,
    pub cost: Option<Money>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DecisionEvent {
    pub piece_id: RecordId,
    pub decision: KeepDecision,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub reasons: Vec<DecisionReason>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ReturnedEvent {
    pub piece_id: RecordId,
    pub refund: Option<Money>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RetiredEvent {
    pub piece_id: RecordId,
    pub reason: RetireReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ValueAssessment {
    pub rating: Rating,
    #[schemars(range(max = 100000))]
    pub wear_count: Option<u32>,
    #[schemars(length(max = 1000))]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Regret {
    #[schemars(length(min = 1, max = 1000))]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct FeedbackEvent {
    /// A piece or prediction id.
    pub target_id: RecordId,
    pub timing: FeedbackTiming,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub region_fit: Vec<RegionFit>,
    pub style: Option<Assessment>,
    pub comfort: Option<Assessment>,
    pub quality: Option<Assessment>,
    pub value: Option<ValueAssessment>,
    pub regret: Option<Regret>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct OutcomeEvent {
    pub prediction_id: RecordId,
    pub piece_id: Option<RecordId>,
    /// When this outcome was judged. A prediction can have one outcome per
    /// stage; a later stage (after wash) does not replace an earlier one.
    pub stage: FeedbackTiming,
    pub result: OutcomeResult,
    /// Actual fit per region; compared with the frozen expectations.
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub region_results: Vec<RegionFit>,
    /// The try-on, wear, wash or feedback events this outcome summarises.
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub based_on_event_ids: Vec<EventId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ContradictionEvent {
    pub status: ContradictionStatus,
    /// The conflicting records, events or evidence.
    #[schemars(length(min = 1, max = 20))]
    pub ids: Vec<LedgerId>,
    #[schemars(length(min = 1, max = 2000))]
    pub description: String,
    /// The open contradiction event this resolves.
    pub resolves_event_id: Option<EventId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct NoteEvent {
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub ids: Vec<LedgerId>,
    #[schemars(length(min = 1, max = 4000))]
    pub text: String,
}

// ---------------------------------------------------------------------------
// Evidence

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A photo showing the owner's body; never exposed outside authenticated MCP.
    BodyPhoto,
    GarmentPhoto,
    Label,
    Receipt,
    SizeChart,
    ProductPage,
    Review,
    Screenshot,
    Note,
    Other,
}

/// Evidence metadata, stored beside the private bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct EvidenceInfo {
    pub kind: EvidenceKind,
    /// When the photo or document was captured (YYYY-MM-DD).
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub captured_on: Option<String>,
    /// Where it came from, e.g. `retailer product page`, `owner's phone`.
    #[schemars(length(max = 500))]
    pub source: Option<String>,
    #[schemars(length(max = 2000))]
    pub source_url: Option<String>,
    #[schemars(length(max = 2000))]
    pub caption: Option<String>,
    pub certainty: Certainty,
    /// Records this evidence supports.
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub links: Vec<RecordId>,
}

impl EvidenceKind {
    /// Retailer and review content is untrusted: evidence, never instructions.
    pub fn is_untrusted(self) -> bool {
        matches!(
            self,
            EvidenceKind::ProductPage | EvidenceKind::Review | EvidenceKind::Screenshot
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_names() {
        for kind in RecordKind::ALL {
            assert_eq!(RecordKind::parse(kind.as_str()), Some(kind));
            assert_eq!(RecordKind::from_prefix(kind.prefix()), Some(kind));
            let encoded = serde_json::to_value(kind).unwrap();
            assert_eq!(encoded, serde_json::json!(kind.as_str()));
        }
        for event in EventType::ALL {
            assert_eq!(EventType::parse(event.as_str()), Some(event));
            let encoded = serde_json::to_value(event).unwrap();
            assert_eq!(encoded, serde_json::json!(event.as_str()));
        }
    }
}
