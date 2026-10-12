//! Service inputs and outputs. They are also the MCP tool contracts: the
//! derived schemas are served by `tools/list` and validate every call.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::fit::RegionComparison;
use crate::model::{EventBody, EvidenceInfo, Provenance, RecordBody};
use crate::state::Link;
use crate::types::{
    ArchiveId, Category, EventId, EvidenceId, IdempotencyKey, LedgerId, Namespace, RecordId,
    Sha256Hex,
};

pub type Object = Map<String, Value>;

// ---------------------------------------------------------------------------
// Writes

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct SaveRecordInput {
    pub namespace: Namespace,
    /// New key per logical write; reuse it with identical input to retry safely.
    pub idempotency_key: IdempotencyKey,
    /// Omit to create. Set with `expectedRevision` to revise a mutable record
    /// (variant, piece, preference, rule, need).
    pub id: Option<RecordId>,
    /// The record's current revision; a stale value is rejected.
    #[schemars(range(min = 1))]
    pub expected_revision: Option<u64>,
    /// Creating an immutable record (measurement, profile snapshot,
    /// prediction) that corrects this one. The old record stays, marked superseded.
    pub supersedes: Option<RecordId>,
    #[schemars(length(min = 1, max = 1000))]
    pub correction_reason: Option<String>,
    pub record: RecordBody,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct AppendEventInput {
    pub namespace: Namespace,
    pub idempotency_key: IdempotencyKey,
    /// When it happened (YYYY-MM-DD, owner's local date); not in the future.
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub occurred_on: String,
    pub event: EventBody,
    #[serde(default)]
    #[schemars(length(max = 20))]
    pub evidence_ids: Vec<EvidenceId>,
    /// Omit when the owner reported it directly: stored as an explicit user statement.
    pub provenance: Option<Provenance>,
    #[schemars(length(max = 4000))]
    pub notes: Option<String>,
    /// Corrects an earlier event of the same type. The original stays, marked superseded.
    pub supersedes_event_id: Option<EventId>,
    #[schemars(length(min = 1, max = 1000))]
    pub correction_reason: Option<String>,
}

/// One record revision as served.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RecordItem {
    pub id: String,
    pub kind: String,
    pub revision: u64,
    /// The ledger revision that wrote this revision.
    pub ledger_revision: u64,
    pub created_revision: u64,
    pub created_at: String,
    pub recorded_at: String,
    /// `current`, `superseded`, a piece's lifecycle status, or a rule's or need's status.
    pub status: String,
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
    pub correction_reason: Option<String>,
    /// The record body exactly as saved (the `record` input).
    pub data: Object,
    /// Server-derived values (normalized millimetres, freeze details, rule cases).
    pub derived: Option<Object>,
    pub links: Vec<Link>,
}

/// One event as served.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct EventItem {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub ledger_revision: u64,
    pub occurred_on: String,
    pub recorded_at: String,
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
    /// The event as appended (occurredOn, event, evidenceIds, provenance, notes, ...).
    pub data: Object,
    /// Server-derived values (forecast comparison, receipt key).
    pub derived: Option<Object>,
    pub links: Vec<Link>,
}

/// Evidence metadata as served (bytes only through `stylist_evidence_get`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct EvidenceItem {
    pub id: String,
    pub sha256: String,
    pub mime_type: String,
    pub size: u64,
    pub ledger_revision: u64,
    pub recorded_at: String,
    /// Retailer pages, reviews and screenshots are untrusted content.
    pub untrusted: bool,
    pub info: Object,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct SaveRecordOutput {
    pub namespace: String,
    /// True when this key was already applied; the original result is returned.
    pub replayed: bool,
    pub ledger_revision: u64,
    pub warnings: Vec<String>,
    pub record: RecordItem,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct AppendEventOutput {
    pub namespace: String,
    pub replayed: bool,
    pub ledger_revision: u64,
    pub warnings: Vec<String>,
    pub event: EventItem,
}

/// Accepted evidence formats; bytes must match the declared type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum EvidenceMime {
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/webp")]
    Webp,
    #[serde(rename = "image/heic")]
    Heic,
    #[serde(rename = "image/heif")]
    Heif,
    #[serde(rename = "application/pdf")]
    Pdf,
    #[serde(rename = "text/plain")]
    Text,
    #[serde(rename = "text/markdown")]
    Markdown,
    #[serde(rename = "application/json")]
    Json,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct StoreEvidenceInput {
    pub namespace: Namespace,
    pub idempotency_key: IdempotencyKey,
    pub mime_type: EvidenceMime,
    /// Base64 bytes, at most 3,000,000 decoded bytes (the MCP request limit is
    /// 4 MiB). Resize photos to about 2048 px. Exactly one of dataBase64 and text.
    #[schemars(length(min = 4, max = 4_194_304))]
    pub data_base64: Option<String>,
    /// Text evidence (a pasted size chart or review); text MIME types only.
    #[schemars(length(min = 1, max = 200_000))]
    pub text: Option<String>,
    pub info: EvidenceInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct StoreEvidenceOutput {
    pub namespace: String,
    pub replayed: bool,
    /// The same bytes were already stored in this namespace; the existing id
    /// and its original metadata are returned and the new metadata is not saved.
    pub deduplicated: bool,
    pub ledger_revision: u64,
    pub warnings: Vec<String>,
    pub evidence: EvidenceItem,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct GetEvidenceInput {
    pub namespace: Namespace,
    pub evidence_id: EvidenceId,
    /// Return the bytes as an embedded resource (default true).
    #[schemars(extend("default" = true))]
    pub include_bytes: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct GetEvidenceOutput {
    pub namespace: String,
    pub evidence: EvidenceItem,
    /// The bytes were re-hashed on read and match `sha256`.
    pub verified: bool,
    /// Always null over MCP: the bytes are the embedded resource
    /// (`omni-stylist-evidence:<id>`, base64 `blob`).
    pub blob: Option<String>,
}

// ---------------------------------------------------------------------------
// Reads

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Collection {
    #[default]
    Records,
    Events,
    Evidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    Newest,
    Oldest,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct QueryInput {
    pub namespace: Namespace,
    #[schemars(extend("default" = "records"))]
    pub collection: Option<Collection>,
    /// Record kinds, event types or evidence kinds (exact).
    #[serde(default)]
    #[schemars(length(max = 12))]
    pub kinds: Vec<crate::types::KindName>,
    /// Exact ids.
    #[serde(default)]
    #[schemars(length(max = 100))]
    pub ids: Vec<LedgerId>,
    /// Items that link to, or are linked from, this id.
    pub linked_to: Option<LedgerId>,
    /// Exact category.
    pub category: Option<Category>,
    /// This category and those below it.
    pub category_prefix: Option<Category>,
    /// Exact brand (case-insensitive), through variants for pieces and predictions.
    #[schemars(length(min = 1, max = 120))]
    pub brand: Option<String>,
    /// Exact served status (`current`, `owned`, `undecided`, `active`, `open`, ...).
    #[serde(default)]
    #[schemars(length(max = 10))]
    pub statuses: Vec<crate::types::KindName>,
    /// Inclusive lower bound on an event's occurredOn or a measurement's observedOn.
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub from: Option<String>,
    #[schemars(transform = omni_mcp_kit::schema::date)]
    pub to: Option<String>,
    /// Include superseded records and events (default false).
    pub include_superseded: Option<bool>,
    /// Records only: every revision of each match, oldest first.
    pub include_history: Option<bool>,
    /// Read the ledger as of this revision. Pass the first page's
    /// snapshotRevision on later pages for a stable listing.
    #[schemars(range(min = 0))]
    pub as_of_revision: Option<u64>,
    #[schemars(extend("default" = "newest"))]
    pub order: Option<Order>,
    #[schemars(description = "Zero-based offset", extend("default" = 0))]
    pub cursor: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 25))]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct QueryOutput {
    pub namespace: String,
    /// The ledger revision this page reflects.
    pub snapshot_revision: u64,
    pub collection: Collection,
    pub records: Vec<RecordItem>,
    pub events: Vec<EventItem>,
    pub evidence: Vec<EvidenceItem>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Focus {
    /// The category being shopped or discussed (matches this and below).
    pub category: Option<Category>,
    #[schemars(length(min = 1, max = 120))]
    pub brand: Option<String>,
    /// A candidate variant: adds its brand and model history.
    pub variant_id: Option<RecordId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ContextInput {
    pub namespace: Namespace,
    pub focus: Option<Focus>,
    /// Reconstruct the context as of this ledger revision.
    #[schemars(range(min = 0))]
    pub as_of_revision: Option<u64>,
    /// Include the runbook text (default true; the version and digest are always returned).
    #[schemars(extend("default" = true))]
    pub include_runbook: Option<bool>,
    /// A specific runbook version (default: current).
    #[schemars(length(min = 1, max = 60))]
    pub runbook_version: Option<String>,
    /// Maximum items per history list (default 15).
    #[schemars(range(min = 1, max = 50), extend("default" = 15))]
    pub limit: Option<u64>,
    /// Re-render exactly the context a token names (its revision, date,
    /// focus, limit and runbook version); the other selectors must be omitted.
    pub context_token: Option<crate::types::ContextToken>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct CompareInput {
    pub namespace: Namespace,
    /// Measurement, piece or variant ids, or `body` for current body measurements.
    #[schemars(length(min = 1, max = 20))]
    pub reference_ids: Vec<String>,
    #[schemars(length(min = 1, max = 20))]
    pub candidate_ids: Vec<String>,
    /// Only this dimension.
    pub dimension: Option<crate::types::Slug>,
    #[schemars(range(min = 0))]
    pub as_of_revision: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct MeasurementRef {
    pub id: String,
    pub subject: String,
    pub subject_id: Option<String>,
    pub dimension: String,
    pub method: String,
    pub value: f64,
    pub unit: String,
    pub value_mm: f64,
    pub observed_on: String,
    pub wash: Option<String>,
    pub endpoints: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ComparedPair {
    pub reference: MeasurementRef,
    pub candidate: MeasurementRef,
    /// `candidate - reference`.
    pub delta_mm: f64,
    pub tolerance_mm: Option<f64>,
    pub caveats: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct IncompatiblePair {
    pub reference: MeasurementRef,
    pub candidate: MeasurementRef,
    pub reason: String,
    pub explanation: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct CompareOutput {
    pub namespace: String,
    pub snapshot_revision: u64,
    pub comparable: Vec<ComparedPair>,
    /// Same dimension, but kept separate: never converted or compared.
    pub incompatible: Vec<IncompatiblePair>,
    /// Dimensions present on only one side.
    pub unmatched: Vec<String>,
}

// ---------------------------------------------------------------------------
// Export, restore and fixtures

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ExportInput {
    pub namespace: Namespace,
    /// Embed evidence bytes in the archive (default true).
    #[schemars(extend("default" = true))]
    pub include_bytes: Option<bool>,
    /// Also return the archive document as an embedded resource when it is at
    /// most 16 MiB (default false).
    #[schemars(extend("default" = false))]
    pub return_document: Option<bool>,
}

/// An archive's manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub archive_id: String,
    pub format: String,
    pub format_version: u64,
    pub namespace: String,
    pub ledger_revision: u64,
    pub exported_at: String,
    /// `export`, `backup` or `pre_purge`.
    pub reason: String,
    pub includes_bytes: bool,
    pub counts: ArchiveCounts,
    /// SHA-256 of each section and of the namespace-independent content.
    pub checksums: Object,
    /// SHA-256 of the archive file.
    pub file_sha256: String,
    pub file_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ArchiveCounts {
    pub records: u64,
    pub record_revisions: u64,
    pub events: u64,
    pub evidence: u64,
    pub evidence_bytes: u64,
    pub idempotency_keys: u64,
    pub relationships: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ExportOutput {
    pub manifest: ArchiveManifest,
    /// The archive was re-read and every checksum verified after writing.
    pub verified: bool,
    /// Whether the document is attached as an embedded resource.
    pub document_attached: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ArchivesListInput {
    /// Only archives of this namespace.
    pub namespace: Option<Namespace>,
    #[schemars(range(min = 1, max = 100), extend("default" = 25))]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ArchivesListOutput {
    /// Newest first.
    pub archives: Vec<ArchiveManifest>,
    pub total: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RestoreInput {
    /// The target: an empty fixture namespace. `real` is restored only by the
    /// operator command.
    pub namespace: Namespace,
    pub archive_id: ArchiveId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RestoreOutput {
    pub namespace: String,
    pub archive_id: String,
    pub source_namespace: String,
    pub ledger_revision: u64,
    pub counts: ArchiveCounts,
    /// Every evidence file was present and matched its digest.
    pub evidence_verified: u64,
    /// The restored namespace re-exports to the archive's content checksum.
    pub verified: bool,
    pub content_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct PurgeInput {
    /// A fixture namespace. `real` cannot be purged.
    pub namespace: Namespace,
    /// Must repeat `namespace` exactly.
    pub confirm_namespace: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct PurgeOutput {
    pub namespace: String,
    /// The pre-purge archive (with evidence bytes) that `stylist_restore` can bring back.
    pub archive: ArchiveManifest,
    pub rows_removed: Object,
    /// Evidence files no namespace references any more, removed from disk.
    pub evidence_files_removed: u64,
}

// ---------------------------------------------------------------------------
// Context

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct Snapshot {
    pub namespace: String,
    /// Cite this as `contextRevision` when freezing a prediction.
    pub ledger_revision: u64,
    pub newest_revision: u64,
    pub generated_at: String,
    /// The date due items and staleness were judged on.
    pub today: String,
    pub runbook_version: String,
    pub runbook_sha256: String,
    /// SHA-256 of this context's content, excluding what changes as the
    /// ledger grows (`generatedAt`, `newestRevision`, the runbook text, the
    /// `historical_snapshot` warning).
    pub context_sha256: String,
    /// Names this exact context: namespace, revision, date, focus, limit and
    /// runbook version, plus `contextSha256`. Pass it as a prediction's
    /// `contextToken`, or to `stylist_context_get` to re-render this context.
    pub context_token: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RunbookOut {
    pub version: String,
    pub sha256: String,
    pub versions: Vec<String>,
    /// The session checklist, always included.
    pub checklist: Vec<String>,
    pub text: Option<String>,
}

/// A current body measurement per dimension and method.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct BodyMeasurementOut {
    pub dimension: String,
    pub method: String,
    pub latest: MeasurementRef,
    pub certainty: String,
    pub source: String,
    /// Older current measurements of the same dimension and method.
    pub earlier: Vec<MeasurementRef>,
    pub stale: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ProfileOut {
    pub latest_snapshot: Option<RecordItem>,
    pub snapshot_count: u64,
    pub body_measurements: Vec<BodyMeasurementOut>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ReferenceOut {
    pub piece: RecordItem,
    pub variant: Option<RecordItem>,
    pub role: String,
    pub categories: Vec<String>,
    pub regions: Vec<String>,
    /// The newest current measurement per dimension and method.
    pub measurements: Vec<MeasurementRef>,
    pub washes: u64,
    pub last_wash_on: Option<String>,
    /// Every measurement listed was taken after at least one wash.
    pub washed_baseline: bool,
    pub alterations: u64,
}

/// One case in the fit history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct HistoryEntry {
    /// `outcome`, `try_on`, `wear`, `feedback`, `wash`, `return`, `regret` or `kept_unassessed`.
    pub kind: String,
    pub occurred_on: String,
    pub event_id: Option<String>,
    pub piece_id: Option<String>,
    pub prediction_id: Option<String>,
    pub variant_id: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub size_label: Option<String>,
    pub category: Option<String>,
    pub regions: Vec<RegionVerdict>,
    pub detail: String,
    pub comparison: Vec<RegionComparison>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RegionVerdict {
    pub region: String,
    pub verdict: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct HistoryOut {
    /// Good outcomes and reports with no adverse region or rating.
    pub successes: Vec<HistoryEntry>,
    /// Missed forecasts, adverse fit or ratings, returns and regrets.
    pub failures: Vec<HistoryEntry>,
    /// Kept without a fit assessment, forecasts without outcomes, mixed reports.
    pub unresolved: Vec<HistoryEntry>,
    pub totals: HistoryTotals,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct HistoryTotals {
    pub successes: u64,
    pub failures: u64,
    pub unresolved: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct BrandHistory {
    pub brand: String,
    pub model: Option<String>,
    pub pieces: u64,
    pub owned: u64,
    pub returned: u64,
    pub retired: u64,
    pub sizes: Vec<String>,
    pub adverse_reports: u64,
    pub positive_reports: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RuleOut {
    pub rule: RecordItem,
    /// In-scope fit cases recorded since the rule's last revision and not in its regression list.
    pub unchecked_cases: Vec<String>,
    /// Unchecked cases with an adverse region, which may contradict the rule.
    pub adverse_unchecked: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct RulesOut {
    pub active: Vec<RuleOut>,
    pub proposed: Vec<RuleOut>,
    /// Active rules with unchecked in-scope cases: test before relying on them.
    pub needs_review: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct PreferencesOut {
    pub current: Vec<RecordItem>,
    /// Preferences revised in the last 365 days, with their previous statement.
    pub recent_changes: Vec<PreferenceChange>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct PreferenceChange {
    pub id: String,
    pub valid_from: String,
    pub statement: String,
    pub previous_valid_from: String,
    pub previous_statement: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ContradictionOut {
    /// `computed` (from the data) or `flagged` (an open contradiction event).
    pub source: String,
    /// `body_measurement`, `garment_measurement`, `fit_report`, `preference` or `flagged`.
    pub kind: String,
    pub description: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct WarningOut {
    pub code: String,
    /// `blocking` means abstain from size advice until resolved.
    pub severity: String,
    pub message: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DueItem {
    /// `return_decision`, `outcome_follow_up`, `post_wash_check` or `need_review`.
    pub kind: String,
    pub id: String,
    pub label: String,
    /// The deadline or review date, when known.
    pub due_on: Option<String>,
    /// Days until `dueOn`; negative when overdue.
    pub days_left: Option<i64>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ContextCounts {
    pub records: Object,
    pub events: u64,
    pub evidence: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct ContextOutput {
    pub snapshot: Snapshot,
    pub runbook: RunbookOut,
    pub focus: Object,
    pub profile: ProfileOut,
    pub references: Vec<ReferenceOut>,
    pub history: HistoryOut,
    pub brand_history: Vec<BrandHistory>,
    pub preferences: PreferencesOut,
    pub rules: RulesOut,
    pub contradictions: Vec<ContradictionOut>,
    pub warnings: Vec<WarningOut>,
    pub due: Vec<DueItem>,
    pub open_needs: Vec<RecordItem>,
    pub counts: ContextCounts,
}

/// Identifies the archive a restore reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveSource {
    Id(ArchiveId),
    Path(std::path::PathBuf),
}

/// Ids accepted by [`CompareInput`] besides measurement, piece and variant ids.
pub const BODY: &str = "body";

/// A [`Sha256Hex`] from a digest.
pub fn sha_hex(digest: &[u8]) -> Sha256Hex {
    Sha256Hex(hex::encode(digest))
}
