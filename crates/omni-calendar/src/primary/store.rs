//! Durable state of the primary calendar: identity pin, sync state, the local
//! mirror, the change feed, write echoes and MCP write operations.

use std::collections::BTreeMap;

use omni_store::cbor::Extra;
use omni_store::entity::Entity;
use serde::{Deserialize, Serialize};

use super::model::Projection;

/// The singleton key of the pin and the sync state.
pub const SINGLETON: &str = "primary";

/// Change rows are kept 30 days, at most [`CHANGE_MAX_ROWS`] rows.
pub const CHANGE_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
pub const CHANGE_MAX_ROWS: usize = 5_000;
/// Write echoes are kept 7 days.
pub const ECHO_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Terminal operations: confirmed and failed 30 days, uncertain 90 days.
pub const OPERATION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
pub const UNCERTAIN_OPERATION_TTL_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// `calendar-primary-pin`: which collection is the primary calendar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryPin {
    pub key: String,
    /// The collection's final path segment (e.g. `work`).
    pub collection_segment: String,
    /// SHA-256 of the full collection path.
    pub path_sha256: String,
    pub pinned_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repinned_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path_sha256: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PrimaryPin {
    const NAME: &'static str = "calendar-primary-pin";
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
}

/// `calendar-primary-state`: sync bookkeeping and the change sequence.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub key: String,
    /// The collection these rows mirror (SHA-256 of its path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_full_sync_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub resource_count: u64,
    /// The last change sequence number assigned.
    #[serde(default)]
    pub change_seq: i64,
    /// Whether the baseline (first) sync completed.
    #[serde(default)]
    pub baselined: bool,
    /// The last change sequence handed to `calendar.event_changed` (or
    /// skipped because nobody was subscribed).
    #[serde(default)]
    pub published_seq: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for SyncState {
    const NAME: &'static str = "calendar-primary-state";
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
}

/// `calendar-primary-resource`: one mirrored resource, keyed by resource name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorResource {
    pub event_id: String,
    pub etag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// The raw iCalendar text (absent when oversize or unparseable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ics: Option<String>,
    pub projection: Projection,
    pub seen_at: i64,
    #[serde(default)]
    pub oversize: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for MirrorResource {
    const NAME: &'static str = "calendar-primary-resource";
    type Key = String;
    fn key(&self) -> String {
        self.event_id.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeOrigin {
    /// The resulting version was written by Omni's calendar tools.
    Omni,
    External,
}

/// `calendar-primary-change`: one detected change, keyed by its zero-padded
/// sequence number (the feed cursor).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRow {
    pub key: String,
    pub seq: i64,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub kind: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Projection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Projection>,
    #[serde(default)]
    pub changed_fields: Vec<String>,
    /// The new ETag (`None` for a deletion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The ETag the mirror held before the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_etag: Option<String>,
    pub origin: ChangeOrigin,
    pub detected_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for ChangeRow {
    const NAME: &'static str = "calendar-primary-change";
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
}

/// The row key of change number `seq` (sorts in sequence order).
pub fn change_key(seq: i64) -> String {
    format!("{seq:016}")
}

/// `calendar-primary-write-echo`: a version Omni's tools wrote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteEcho {
    pub key: String,
    pub event_id: String,
    /// The written ETag, or `deleted`.
    pub etag: String,
    pub operation_key: String,
    pub created_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for WriteEcho {
    const NAME: &'static str = "calendar-primary-write-echo";
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
}

pub fn echo_key(event_id: &str, etag: &str) -> String {
    format!("{event_id}:{etag}")
}

/// The ETag marker of a deletion echo.
pub const DELETED: &str = "deleted";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationState {
    Reserved,
    Confirmed,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Put,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    Planned,
    Sending,
    Acknowledged,
    Verified,
    Rejected,
    Uncertain,
}

/// One planned remote write.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStep {
    pub kind: StepKind,
    pub event_id: String,
    /// `if-match:<etag>` or `if-none-match`.
    pub precondition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_sha256: Option<String>,
    pub state: StepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The resource before the write (for reconciliation), dropped when terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_ics: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationErrorRecord {
    pub code: String,
    pub message: String,
}

/// `calendar-mcp-operation`: one idempotent write tool call, keyed by the
/// SHA-256 of its idempotency key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub key_hash: String,
    pub idempotency_key: String,
    pub fingerprint: String,
    pub tool: String,
    pub state: OperationState,
    pub steps: Vec<OperationStep>,
    /// Exact bodies to send, by step index; dropped when terminal.
    #[serde(default)]
    pub planned_bodies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<OperationErrorRecord>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for OperationRecord {
    const NAME: &'static str = "calendar-mcp-operation";
    type Key = String;
    fn key(&self) -> String {
        self.key_hash.clone()
    }
}

impl OperationRecord {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            OperationState::Confirmed | OperationState::Failed | OperationState::Uncertain
        )
    }

    /// Drops bodies no longer needed once terminal (uncertain operations keep
    /// theirs for reconciliation).
    pub fn compact(&mut self) {
        if matches!(
            self.state,
            OperationState::Confirmed | OperationState::Failed
        ) {
            self.planned_bodies.clear();
            for step in &mut self.steps {
                step.before_ics = None;
            }
        }
    }

    pub fn ttl_ms(&self) -> Option<i64> {
        match self.state {
            OperationState::Reserved => None,
            OperationState::Uncertain => Some(UNCERTAIN_OPERATION_TTL_MS),
            OperationState::Confirmed | OperationState::Failed => Some(OPERATION_TTL_MS),
        }
    }
}
