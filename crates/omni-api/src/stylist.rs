//! `GET /api/stylist?namespace=real`: a read-only overview of the stylist
//! ledger for the web UI (see docs/stylist.md). Evidence bytes and photos are
//! never part of it; they stay behind authenticated MCP.

use serde::{Deserialize, Serialize};

/// A namespace with its ledger revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistNamespace {
    pub namespace: String,
    pub revision: f64,
    pub fixture: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistRunbook {
    pub version: String,
    pub sha256: String,
    pub checklist: Vec<String>,
}

/// Current records and other rows, by kind.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistCounts {
    pub variants: f64,
    pub pieces: f64,
    pub measurements: f64,
    pub profile_snapshots: f64,
    pub preferences: f64,
    pub predictions: f64,
    pub rules: f64,
    pub needs: f64,
    pub events: f64,
    pub evidence: f64,
}

/// A completeness, freshness or consistency warning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistWarning {
    pub code: String,
    /// `blocking`, `warning` or `info`.
    pub severity: String,
    pub message: String,
}

/// A return decision, outcome follow-up, post-wash check or need review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistDue {
    /// `return_decision`, `outcome_follow_up`, `post_wash_check` or `need_review`.
    pub kind: String,
    pub id: String,
    pub label: String,
    pub due_on: Option<String>,
    pub days_left: Option<f64>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistContradiction {
    /// `computed` or `flagged`.
    pub source: String,
    pub kind: String,
    pub description: String,
}

/// One measurement as shown: millimetres plus the value as taken.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistMeasurement {
    pub id: String,
    pub dimension: String,
    /// `circumference`, `flat_width`, `linear` or `curve`.
    pub method: String,
    pub value_mm: f64,
    /// The value and unit as recorded, e.g. `21.25 in`.
    pub original: String,
    pub observed_on: String,
    pub endpoints: String,
    /// Garments only: `washed`, `unwashed` or `unknown`.
    pub wash: Option<String>,
}

/// The newest body measurement per dimension and method.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistBodyMeasurement {
    pub latest: StylistMeasurement,
    /// `explicit`, `inferred` or `unknown`.
    pub certainty: String,
    pub source: String,
    /// Older readings of the same dimension and method.
    pub earlier: Vec<StylistMeasurement>,
    pub stale: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistAttribute {
    pub name: String,
    /// The number and unit, or the text.
    pub value: String,
    pub certainty: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistProfile {
    /// The newest snapshot's date, when there is one.
    pub observed_on: Option<String>,
    pub attributes: Vec<StylistAttribute>,
    /// `region: note` lines.
    pub sensitivities: Vec<String>,
    pub body: Vec<StylistBodyMeasurement>,
}

/// One owned or tried piece.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistPiece {
    pub id: String,
    pub label: String,
    pub category: String,
    /// `owned`, `undecided`, `return_pending`, `returned` or `retired`.
    pub status: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub size_label: Option<String>,
    pub colour: Option<String>,
    /// e.g. `100% cotton`.
    pub fabric: Option<String>,
    /// `proven`, `failed`, `mixed` or `untested`.
    pub fit: String,
    /// `baseline`, `too_small`, `too_large` or `partial`, when a reference.
    pub reference_role: Option<String>,
    pub reference_regions: Vec<String>,
    pub washes: f64,
    pub alterations: f64,
    /// The newest measurement per dimension and method.
    pub measurements: Vec<StylistMeasurement>,
    pub last_event_on: Option<String>,
    pub return_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistRegion {
    pub region: String,
    pub verdict: String,
}

/// A case in the fit history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistCase {
    /// `outcome`, `try_on`, `wear`, `feedback`, `wash`, `return`, `regret`,
    /// `kept_unassessed` or `forecast_without_outcome`.
    pub kind: String,
    pub occurred_on: String,
    /// Brand, model and size when known, else the piece label.
    pub subject: String,
    pub category: Option<String>,
    pub detail: String,
    pub regions: Vec<StylistRegion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistHistory {
    pub successes: Vec<StylistCase>,
    pub failures: Vec<StylistCase>,
    pub unresolved: Vec<StylistCase>,
    pub success_total: f64,
    pub failure_total: f64,
    pub unresolved_total: f64,
}

/// One region of a forecast scored against an outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistScore {
    pub region: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
    /// `exact`, `close`, `miss`, `unknown`, `not_reported` or `not_forecast`.
    #[serde(rename = "match")]
    pub matched: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistOutcome {
    /// `immediate`, `after_wear`, `after_wash` or `delayed`.
    pub stage: String,
    /// `matched`, `partial`, `missed`, `unresolved` or `not_purchased`.
    pub result: String,
    pub occurred_on: String,
    pub scores: Vec<StylistScore>,
}

/// A frozen forecast and how it turned out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistPrediction {
    pub id: String,
    pub frozen_at: String,
    /// Brand, model and size.
    pub subject: String,
    pub category: String,
    /// `recommend`, `recommend_with_caution` or `abstain`.
    pub decision: String,
    pub basis: String,
    pub computed_basis: Option<String>,
    pub return_risk: String,
    pub failure_mode: String,
    pub expectations: Vec<StylistRegion>,
    pub evidence_gaps: Vec<String>,
    /// The cited context was re-rendered and matched at freeze time.
    pub context_verified: bool,
    pub runbook_version: Option<String>,
    pub outcomes: Vec<StylistOutcome>,
    pub superseded: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistRule {
    pub id: String,
    pub statement: String,
    /// `proposed`, `active` or `retired`.
    pub status: String,
    pub basis: String,
    /// e.g. `Uniqlo · U Crew Neck T-Shirt · body_length`.
    pub scope: String,
    pub in_scope_cases: f64,
    pub unchecked_cases: f64,
    pub needs_review: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistPreference {
    pub id: String,
    pub category: String,
    pub aspect: String,
    pub region: Option<String>,
    pub statement: String,
    /// `require`, `prefer`, `avoid` or `reject`.
    pub strength: String,
    pub valid_from: String,
    pub previous: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistNeed {
    pub id: String,
    pub category: String,
    /// `gap`, `replacement`, `restock` or `upgrade`.
    pub reason: String,
    /// `high`, `medium` or `low`.
    pub priority: String,
    pub description: String,
    /// `open`, `paused`, `satisfied` or `dropped`.
    pub status: String,
    pub review_after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistArchive {
    pub archive_id: String,
    /// `export`, `backup` or `pre_purge`.
    pub reason: String,
    pub ledger_revision: f64,
    pub exported_at: String,
    pub includes_bytes: bool,
    pub file_bytes: f64,
    pub evidence: f64,
}

/// `GET /api/stylist`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StylistOverview {
    pub namespace: String,
    pub namespaces: Vec<StylistNamespace>,
    pub ledger_revision: f64,
    pub generated_at: String,
    pub today: String,
    pub runbook: StylistRunbook,
    pub counts: StylistCounts,
    pub warnings: Vec<StylistWarning>,
    pub due: Vec<StylistDue>,
    pub contradictions: Vec<StylistContradiction>,
    pub profile: StylistProfile,
    pub wardrobe: Vec<StylistPiece>,
    pub history: StylistHistory,
    pub predictions: Vec<StylistPrediction>,
    pub rules: Vec<StylistRule>,
    pub preferences: Vec<StylistPreference>,
    pub needs: Vec<StylistNeed>,
    pub archives: Vec<StylistArchive>,
}
