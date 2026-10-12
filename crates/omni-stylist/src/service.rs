//! The stylist service: transactional writes with idempotency, reads over a
//! consistent snapshot, private evidence, comparisons.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use jiff::civil::Date;
use omni_core::clock::SharedClock;
use omni_store::{DocOps as _, Store};
use rusqlite::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::api::{
    AppendEventInput, AppendEventOutput, BODY, Collection, CompareInput, CompareOutput,
    ComparedPair, EventItem, EvidenceItem, GetEvidenceInput, GetEvidenceOutput, IncompatiblePair,
    MeasurementRef, Object, Order, QueryInput, QueryOutput, RecordItem, SaveRecordInput,
    SaveRecordOutput, StoreEvidenceInput, StoreEvidenceOutput,
};
use crate::error::{StylistError, invalid, io, not_found, reject};
use crate::evidence::{self, PrivateDir};
use crate::ledger::{self, EventRow, EvidenceRow, IdempotencyRow, RecordRow};
use crate::model::{MeasurementRecord, RecordBody, RecordKind, SubjectKind};
use crate::state::{Event, Evidence, LedgerState, Record, enum_text};
use crate::types::{EvidenceId, Namespace, new_id, parse_date};
use crate::units::{self, Comparison};
use crate::{runbook, validate};

pub const TOOL_SAVE: &str = "stylist_record_save";
pub const TOOL_EVENT: &str = "stylist_event_append";
pub const TOOL_EVIDENCE: &str = "stylist_evidence_store";

const LOG: &str = "Stylist";

/// The stylist ledger over the docstore and the private directory.
#[derive(Clone)]
pub struct Stylist {
    pub(crate) store: Store,
    pub(crate) clock: SharedClock,
    pub(crate) tz: jiff::tz::TimeZone,
    pub(crate) private: PrivateDir,
    /// Serializes evidence file writes with file cleanup, so a cleanup never
    /// removes a file whose metadata row is about to commit.
    pub(crate) evidence_lock: Arc<tokio::sync::Mutex<()>>,
    ready: Arc<tokio::sync::OnceCell<()>>,
}

/// An ISO timestamp for epoch milliseconds.
pub fn iso(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms).map_or_else(|_| ms.to_string(), |t| t.to_string())
}

/// JSON with object keys sorted at every level, for digests.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), canonical(&map[key]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// SHA-256 of the canonical JSON of `value`.
pub fn digest_json(value: &Value) -> String {
    evidence::sha256_hex(canonical(value).to_string().as_bytes())
}

/// The idempotency fingerprint: the input without its key.
fn fingerprint<T: Serialize>(input: &T) -> Result<String, StylistError> {
    let mut value = serde_json::to_value(input).map_err(|e| invalid(e.to_string()))?;
    if let Value::Object(map) = &mut value {
        map.shift_remove("idempotencyKey");
    }
    Ok(digest_json(&value))
}

fn to_object(value: &Value) -> Object {
    match value {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<String, StylistError> {
    serde_json::to_string(value).map_err(|e| StylistError::Corrupt(e.to_string()))
}

/// Drops null and empty-array fields at every level: absent and null decode
/// to the same typed value, so stored bodies read back as they were sent.
pub fn compact(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
                .map(|(k, v)| (k, compact(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(compact).collect()),
        other => other,
    }
}

/// The stored text of a body: compact JSON.
fn body_json<T: Serialize>(value: &T) -> Result<String, StylistError> {
    let value = serde_json::to_value(value).map_err(|e| StylistError::Corrupt(e.to_string()))?;
    to_json(&compact(value))
}

/// The recorded result for `key`, or `idempotency_key_reused` when the key
/// was used for different input.
fn replay<T: DeserializeOwned>(
    conn: &Connection,
    namespace: &str,
    key: &str,
    tool: &str,
    fingerprint: &str,
) -> Result<Option<T>, StylistError> {
    let Some(row) = ledger::idempotency(conn, namespace, key)? else {
        return Ok(None);
    };
    if row.tool != tool || row.fingerprint != fingerprint {
        return Err(reject(
            "idempotency_key_reused",
            format!(
                "idempotency key {key} was already used by {} with different input at ledger revision {}; use a new key for a new write",
                row.tool, row.ledger_revision
            ),
        ));
    }
    let mut value: Value = serde_json::from_str(&row.result)
        .map_err(|e| StylistError::Corrupt(format!("idempotency result {key}: {e}")))?;
    if let Value::Object(map) = &mut value {
        map.insert("replayed".into(), Value::Bool(true));
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| StylistError::Corrupt(format!("idempotency result {key}: {e}")))
}

pub fn record_item(state: &LedgerState, record: &Record) -> RecordItem {
    RecordItem {
        id: record.id.as_str().to_owned(),
        kind: record.kind.as_str().to_owned(),
        revision: record.revision,
        ledger_revision: record.ledger_revision,
        created_revision: record.created_revision,
        created_at: iso(record.created_at),
        recorded_at: iso(record.recorded_at),
        status: state.record_status(record),
        supersedes: record.supersedes.as_ref().map(|id| id.as_str().to_owned()),
        superseded_by: state
            .record_superseded_by(record.id.as_str())
            .map(str::to_owned),
        correction_reason: record.correction_reason.clone(),
        data: to_object(&record.payload),
        derived: record.derived.as_ref().map(to_object),
        links: state.record_links(record),
    }
}

/// An item for one historical revision of `record`.
fn revision_item(
    state: &LedgerState,
    record: &Record,
    row: &RecordRow,
) -> Result<RecordItem, StylistError> {
    let corrupt = |e: serde_json::Error| {
        StylistError::Corrupt(format!("record {} revision {}: {e}", row.id, row.revision))
    };
    let payload: Value = serde_json::from_str(&row.payload).map_err(corrupt)?;
    let body: RecordBody = serde_json::from_value(payload.clone()).map_err(corrupt)?;
    let derived: Option<Value> = row
        .derived
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(corrupt)?;
    let mut item = record_item(state, record);
    item.revision = row.revision;
    item.ledger_revision = row.ledger_revision;
    item.recorded_at = iso(row.recorded_at);
    item.data = to_object(&payload);
    item.derived = derived.as_ref().map(to_object);
    if row.revision != record.revision {
        item.status = "previous_revision".to_owned();
        let as_saved = Record {
            body,
            ..record.clone()
        };
        item.links = state.record_links(&as_saved);
    }
    Ok(item)
}

pub fn event_item(state: &LedgerState, event: &Event) -> EventItem {
    EventItem {
        id: event.id.as_str().to_owned(),
        event_type: event.event_type.as_str().to_owned(),
        ledger_revision: event.ledger_revision,
        occurred_on: event.occurred_on.clone(),
        recorded_at: iso(event.recorded_at),
        supersedes: event
            .envelope
            .supersedes_event_id
            .as_ref()
            .map(|id| id.as_str().to_owned()),
        superseded_by: state
            .event_superseded_by(event.id.as_str())
            .map(str::to_owned),
        data: to_object(&event.payload),
        derived: event.derived.as_ref().map(to_object),
        links: state.event_links(event),
    }
}

pub fn evidence_item(evidence: &Evidence) -> EvidenceItem {
    EvidenceItem {
        id: evidence.id.as_str().to_owned(),
        sha256: evidence.sha256.clone(),
        mime_type: evidence.mime_type.clone(),
        size: evidence.size,
        ledger_revision: evidence.ledger_revision,
        recorded_at: iso(evidence.recorded_at),
        untrusted: evidence.info.kind.is_untrusted(),
        info: to_object(&evidence.payload),
        links: LedgerState::evidence_links(evidence),
    }
}

/// The category a record belongs to, directly or through its variant.
pub fn record_category(state: &LedgerState, record: &Record) -> Option<String> {
    match &record.body {
        RecordBody::Variant(v) => Some(v.category.as_str().to_owned()),
        RecordBody::Piece(p) => Some(p.category.as_str().to_owned()),
        RecordBody::Preference(p) => Some(p.category.as_str().to_owned()),
        RecordBody::Need(n) => Some(n.category.as_str().to_owned()),
        RecordBody::Prediction(p) => state
            .variant(p.variant_id.as_str())
            .map(|v| v.category.as_str().to_owned()),
        RecordBody::Measurement(m) => match (&m.subject.kind, &m.subject.id) {
            (SubjectKind::Piece, Some(id)) => state
                .piece(id.as_str())
                .map(|p| p.category.as_str().to_owned()),
            (SubjectKind::VariantPublished, Some(id)) => state
                .variant(id.as_str())
                .map(|v| v.category.as_str().to_owned()),
            _ => None,
        },
        RecordBody::Rule(r) => r.scope.category.as_ref().map(|c| c.as_str().to_owned()),
        RecordBody::ProfileSnapshot(_) => None,
    }
}

/// The brand a record belongs to, directly or through its variant.
pub fn record_brand(state: &LedgerState, record: &Record) -> Option<String> {
    match &record.body {
        RecordBody::Variant(v) => Some(v.brand.clone()),
        RecordBody::Rule(r) => r.scope.brand.clone(),
        RecordBody::Measurement(m) => m
            .subject
            .id
            .as_ref()
            .and_then(|id| state.variant_of(id.as_str()))
            .map(|(_, v)| v.brand.clone()),
        _ => state
            .variant_of(record.id.as_str())
            .map(|(_, v)| v.brand.clone()),
    }
}

/// The date a record is filed under for `from`/`to` filters.
fn record_date(record: &Record) -> String {
    match &record.body {
        RecordBody::Measurement(m) => m.observed_on.clone(),
        RecordBody::ProfileSnapshot(p) => p.observed_on.clone(),
        RecordBody::Preference(p) => p.valid_from.clone(),
        _ => iso(record.created_at).chars().take(10).collect(),
    }
}

fn event_subject_variant<'a>(
    state: &'a LedgerState,
    event: &Event,
) -> Option<&'a crate::model::VariantRecord> {
    let body = &event.envelope.event;
    body.piece_id()
        .or_else(|| body.prediction_id())
        .and_then(|id| state.variant_of(id.as_str()))
        .map(|(_, v)| v)
}

fn in_range(date: &str, from: Option<&String>, to: Option<&String>) -> bool {
    from.is_none_or(|f| date >= f.as_str()) && to.is_none_or(|t| date <= t.as_str())
}

/// A measurement's reference form.
pub fn measurement_ref(record: &Record, m: &MeasurementRecord) -> MeasurementRef {
    MeasurementRef {
        id: record.id.as_str().to_owned(),
        subject: enum_text(&m.subject.kind),
        subject_id: m.subject.id.as_ref().map(|id| id.as_str().to_owned()),
        dimension: m.dimension.as_str().to_owned(),
        method: enum_text(&m.method),
        value: m.value,
        unit: enum_text(&m.unit),
        value_mm: units::to_mm(m.value, m.unit),
        observed_on: m.observed_on.clone(),
        wash: m.garment_state.as_ref().map(|s| enum_text(&s.wash)),
        endpoints: m.endpoints.clone(),
    }
}

impl Stylist {
    pub fn new(
        store: Store,
        clock: SharedClock,
        tz: jiff::tz::TimeZone,
        private_root: std::path::PathBuf,
    ) -> Self {
        Self {
            store,
            clock,
            tz,
            private: PrivateDir::new(private_root),
            evidence_lock: Arc::new(tokio::sync::Mutex::new(())),
            ready: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// Creates the tables and the private directory once per process; a
    /// failure is retried on the next call.
    pub async fn ready(&self) -> Result<(), StylistError> {
        self.ready
            .get_or_try_init(|| async {
                self.store
                    .write(|tx| ledger::ensure_schema(tx.connection()))
                    .await?;
                let private = self.private.clone();
                tokio::task::spawn_blocking(move || private.ensure())
                    .await
                    .map_err(io)?
            })
            .await
            .map(|_| ())
    }

    pub fn private(&self) -> &PrivateDir {
        &self.private
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The owner's local date.
    pub fn today(&self) -> Date {
        self.date_of(self.clock.now_ms())
    }

    /// The owner's local date at epoch milliseconds `ms`.
    pub fn date_of(&self, ms: i64) -> Date {
        jiff::Timestamp::from_millisecond(ms)
            .map(|t| t.to_zoned(self.tz.clone()).date())
            .unwrap_or_default()
    }

    /// Loads `namespace` as of `as_of` on the store thread.
    pub async fn state(
        &self,
        namespace: &Namespace,
        as_of: Option<u64>,
    ) -> Result<LedgerState, StylistError> {
        self.ready().await?;
        let namespace = namespace.clone();
        self.store
            .read(move |docs| Ok(LedgerState::load(docs.connection(), &namespace, as_of)))
            .await?
    }

    pub async fn save_record(
        &self,
        input: SaveRecordInput,
    ) -> Result<SaveRecordOutput, StylistError> {
        self.ready().await?;
        let fingerprint = fingerprint(&input)?;
        let today = self.today();
        self.store
            .write(move |tx| -> Result<SaveRecordOutput, StylistError> {
                let conn = tx.connection();
                let now = tx.now_ms();
                let namespace = input.namespace.as_str();
                if let Some(output) = replay(
                    conn,
                    namespace,
                    input.idempotency_key.as_str(),
                    TOOL_SAVE,
                    &fingerprint,
                )? {
                    return Ok(output);
                }
                let state = LedgerState::load(conn, &input.namespace, None)?;
                let plan = validate::plan_record(&state, &input, today)?;
                let revision = ledger::next_revision(conn, namespace, now)?;
                let mut derived = plan.derived.unwrap_or_default();
                if plan.kind == RecordKind::Prediction {
                    derived.insert("frozenAtRevision".into(), json!(revision));
                    derived.insert("frozenAt".into(), json!(iso(now)));
                    derived.insert("runbookVersion".into(), json!(runbook::current().version));
                    derived.insert("runbookSha256".into(), json!(runbook::current().sha256()));
                }
                let row = RecordRow {
                    id: plan.id.as_str().to_owned(),
                    revision: plan.revision,
                    kind: plan.kind.as_str().to_owned(),
                    ledger_revision: revision,
                    recorded_at: now,
                    supersedes: input.supersedes.as_ref().map(|id| id.as_str().to_owned()),
                    correction_reason: input.correction_reason.clone(),
                    payload: body_json(&plan.body)?,
                    derived: (!derived.is_empty())
                        .then(|| to_json(&derived))
                        .transpose()?,
                };
                ledger::insert_record(conn, namespace, &row)?;
                let state = LedgerState::load(conn, &input.namespace, None)?;
                let record = state
                    .record(plan.id.as_str())
                    .ok_or_else(|| StylistError::Corrupt("saved record not readable".to_owned()))?;
                let output = SaveRecordOutput {
                    namespace: namespace.to_owned(),
                    replayed: false,
                    ledger_revision: revision,
                    warnings: plan.warnings,
                    record: record_item(&state, record),
                };
                ledger::insert_idempotency(
                    conn,
                    namespace,
                    &IdempotencyRow {
                        key: input.idempotency_key.as_str().to_owned(),
                        tool: TOOL_SAVE.to_owned(),
                        fingerprint,
                        ledger_revision: revision,
                        recorded_at: now,
                        result: to_json(&output)?,
                    },
                )?;
                tracing::info!(
                    target: LOG,
                    "Saved {} {} revision {} in {namespace} (ledger revision {revision})",
                    plan.kind.as_str(),
                    plan.id,
                    plan.revision
                );
                Ok(output)
            })
            .await
    }

    pub async fn append_event(
        &self,
        input: AppendEventInput,
    ) -> Result<AppendEventOutput, StylistError> {
        self.ready().await?;
        let fingerprint = fingerprint(&input)?;
        let today = self.today();
        self.store
            .write(move |tx| -> Result<AppendEventOutput, StylistError> {
                let conn = tx.connection();
                let now = tx.now_ms();
                let namespace = input.namespace.as_str();
                if let Some(output) = replay(
                    conn,
                    namespace,
                    input.idempotency_key.as_str(),
                    TOOL_EVENT,
                    &fingerprint,
                )? {
                    return Ok(output);
                }
                let state = LedgerState::load(conn, &input.namespace, None)?;
                let plan = validate::plan_event(&state, &input, today)?;
                let revision = ledger::next_revision(conn, namespace, now)?;
                let row = EventRow {
                    id: plan.id.as_str().to_owned(),
                    event_type: plan.envelope.event.event_type().as_str().to_owned(),
                    ledger_revision: revision,
                    occurred_on: plan.envelope.occurred_on.clone(),
                    recorded_at: now,
                    supersedes: plan
                        .envelope
                        .supersedes_event_id
                        .as_ref()
                        .map(|id| id.as_str().to_owned()),
                    payload: body_json(&plan.envelope)?,
                    derived: plan.derived.as_ref().map(to_json).transpose()?,
                };
                ledger::insert_event(conn, namespace, &row)?;
                let state = LedgerState::load(conn, &input.namespace, None)?;
                let event = state.event(plan.id.as_str()).ok_or_else(|| {
                    StylistError::Corrupt("appended event not readable".to_owned())
                })?;
                let output = AppendEventOutput {
                    namespace: namespace.to_owned(),
                    replayed: false,
                    ledger_revision: revision,
                    warnings: plan.warnings,
                    event: event_item(&state, event),
                };
                ledger::insert_idempotency(
                    conn,
                    namespace,
                    &IdempotencyRow {
                        key: input.idempotency_key.as_str().to_owned(),
                        tool: TOOL_EVENT.to_owned(),
                        fingerprint,
                        ledger_revision: revision,
                        recorded_at: now,
                        result: to_json(&output)?,
                    },
                )?;
                tracing::info!(
                    target: LOG,
                    "Appended {} event {} in {namespace} (ledger revision {revision})",
                    row.event_type,
                    plan.id
                );
                Ok(output)
            })
            .await
    }

    pub async fn store_evidence(
        &self,
        input: StoreEvidenceInput,
    ) -> Result<StoreEvidenceOutput, StylistError> {
        self.ready().await?;
        let bytes = match (&input.data_base64, &input.text) {
            (Some(data), None) => STANDARD
                .decode(data.trim())
                .map_err(|_| invalid("dataBase64 is not valid base64"))?,
            (None, Some(text)) => {
                if !matches!(
                    input.mime_type,
                    crate::api::EvidenceMime::Text
                        | crate::api::EvidenceMime::Markdown
                        | crate::api::EvidenceMime::Json
                ) {
                    return Err(invalid(
                        "text is only for text/plain, text/markdown or application/json",
                    ));
                }
                text.as_bytes().to_vec()
            }
            _ => return Err(invalid("provide exactly one of dataBase64 and text")),
        };
        evidence::validate_bytes(input.mime_type, &bytes)?;
        if let Some(captured) = &input.info.captured_on {
            parse_date(captured).map_err(|e| invalid(format!("info.capturedOn: {e}")))?;
        }
        let fingerprint = fingerprint(&input)?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let _guard = self.evidence_lock.lock().await;
        let private = self.private.clone();
        let stored = tokio::task::spawn_blocking(move || private.put(&bytes))
            .await
            .map_err(io)??;
        let digest = stored.digest.clone();
        let result = self
            .store
            .write(move |tx| -> Result<StoreEvidenceOutput, StylistError> {
                let conn = tx.connection();
                let now = tx.now_ms();
                let namespace = input.namespace.as_str();
                if let Some(output) = replay(
                    conn,
                    namespace,
                    input.idempotency_key.as_str(),
                    TOOL_EVIDENCE,
                    &fingerprint,
                )? {
                    return Ok(output);
                }
                let state = LedgerState::load(conn, &input.namespace, None)?;
                for id in &input.info.links {
                    if state.record(id.as_str()).is_none() {
                        return Err(not_found(format!(
                            "no record {id} in namespace {namespace}"
                        )));
                    }
                }
                let (output, revision) = if let Some(existing) =
                    ledger::evidence_by_hash(conn, namespace, &digest)?
                {
                    let evidence = state.evidence.get(&existing).ok_or_else(|| {
                        StylistError::Corrupt(format!("evidence {existing} not readable"))
                    })?;
                    let revision = state.current_revision;
                    (
                        StoreEvidenceOutput {
                            namespace: namespace.to_owned(),
                            replayed: false,
                            deduplicated: true,
                            ledger_revision: revision,
                            warnings: vec![format!(
                                "these bytes are already stored as {existing} (kind {}); its original metadata is kept and this call's info (kind {}) was not saved; link records to {existing} instead",
                                enum_text(&evidence.info.kind),
                                enum_text(&input.info.kind)
                            )],
                            evidence: evidence_item(evidence),
                        },
                        revision,
                    )
                } else {
                    let revision = ledger::next_revision(conn, namespace, now)?;
                    let id = EvidenceId(new_id("evd"));
                    ledger::insert_evidence(
                        conn,
                        namespace,
                        &EvidenceRow {
                            id: id.as_str().to_owned(),
                            sha256: digest.clone(),
                            mime_type: evidence::mime_str(input.mime_type).to_owned(),
                            size,
                            ledger_revision: revision,
                            recorded_at: now,
                            payload: body_json(&input.info)?,
                        },
                    )?;
                    let state = LedgerState::load(conn, &input.namespace, None)?;
                    let evidence = state.evidence.get(id.as_str()).ok_or_else(|| {
                        StylistError::Corrupt("stored evidence not readable".to_owned())
                    })?;
                    (
                        StoreEvidenceOutput {
                            namespace: namespace.to_owned(),
                            replayed: false,
                            deduplicated: false,
                            ledger_revision: revision,
                            warnings: Vec::new(),
                            evidence: evidence_item(evidence),
                        },
                        revision,
                    )
                };
                ledger::insert_idempotency(
                    conn,
                    namespace,
                    &IdempotencyRow {
                        key: input.idempotency_key.as_str().to_owned(),
                        tool: TOOL_EVIDENCE.to_owned(),
                        fingerprint,
                        ledger_revision: revision,
                        recorded_at: now,
                        result: to_json(&output)?,
                    },
                )?;
                tracing::info!(
                    target: LOG,
                    "Stored evidence {} in {namespace} ({} bytes, deduplicated: {})",
                    output.evidence.id,
                    output.evidence.size,
                    output.deduplicated
                );
                Ok(output)
            })
            .await;
        if result.is_err() && stored.created {
            // A rejected upload removes the file it created, unless a row uses it.
            self.remove_created(vec![stored.digest]).await;
        }
        result
    }

    /// Deletes files this process just created for `digests` when no
    /// namespace references them. Callers hold `evidence_lock`.
    pub(crate) async fn remove_created(&self, digests: Vec<String>) {
        let referenced = self
            .store
            .read(|docs| Ok(ledger::all_evidence_hashes(docs.connection())))
            .await;
        let Ok(Ok(referenced)) = referenced else {
            return;
        };
        let keep: std::collections::HashSet<String> = referenced.into_iter().collect();
        let candidates: std::collections::HashSet<String> = digests.into_iter().collect();
        let private = self.private.clone();
        let removed =
            tokio::task::spawn_blocking(move || private.remove_digests(&candidates, &keep)).await;
        if let Ok(Err(error)) = removed {
            tracing::warn!(target: LOG, "Could not remove a rejected evidence file: {}", error.tool_text());
        }
    }

    /// Evidence metadata and, when requested, its verified bytes.
    pub async fn get_evidence(
        &self,
        input: GetEvidenceInput,
    ) -> Result<GetEvidenceOutput, StylistError> {
        let state = self.state(&input.namespace, None).await?;
        let evidence = state
            .evidence
            .get(input.evidence_id.as_str())
            .ok_or_else(|| {
                not_found(format!(
                    "no evidence {} in namespace {}",
                    input.evidence_id, input.namespace
                ))
            })?;
        let item = evidence_item(evidence);
        let blob = if input.include_bytes.unwrap_or(true) {
            let private = self.private.clone();
            let digest = evidence.sha256.clone();
            let bytes = tokio::task::spawn_blocking(move || private.get(&digest))
                .await
                .map_err(io)??;
            Some(STANDARD.encode(bytes))
        } else {
            None
        };
        Ok(GetEvidenceOutput {
            namespace: input.namespace.as_str().to_owned(),
            evidence: item,
            verified: blob.is_some(),
            blob,
        })
    }

    pub async fn query(&self, input: QueryInput) -> Result<QueryOutput, StylistError> {
        let state = self.state(&input.namespace, input.as_of_revision).await?;
        query_state(&state, &input)
    }

    pub async fn compare(&self, input: CompareInput) -> Result<CompareOutput, StylistError> {
        let state = self.state(&input.namespace, input.as_of_revision).await?;
        compare_state(&state, &input)
    }
}

/// Filters and pages `state` for `input`.
pub fn query_state(state: &LedgerState, input: &QueryInput) -> Result<QueryOutput, StylistError> {
    let collection = input.collection.unwrap_or_default();
    let include_superseded = input.include_superseded.unwrap_or(false);
    let related = input
        .linked_to
        .as_ref()
        .map(|id| state.related_ids(id.as_str()));
    let ids: std::collections::HashSet<&str> = input.ids.iter().map(|id| id.as_str()).collect();
    let brand = input.brand.as_ref().map(|b| b.trim().to_lowercase());
    let category_ok = |category: Option<String>| {
        input
            .category
            .as_ref()
            .is_none_or(|c| category.as_deref() == Some(c.as_str()))
            && input.category_prefix.as_ref().is_none_or(|prefix| {
                category.is_some_and(|c| crate::types::Category(c).is_under(prefix.as_str()))
            })
    };
    let brand_ok = |b: Option<String>| {
        brand
            .as_ref()
            .is_none_or(|want| b.is_some_and(|b| b.trim().to_lowercase() == *want))
    };
    let id_ok = |id: &str| {
        (ids.is_empty() || ids.contains(id)) && related.as_ref().is_none_or(|r| r.contains(id))
    };
    let kind_ok =
        |kind: &str| input.kinds.is_empty() || input.kinds.iter().any(|k| k.as_str() == kind);
    let status_ok = |status: &str| {
        input.statuses.is_empty() || input.statuses.iter().any(|s| s.as_str() == status)
    };
    for kind in &input.kinds {
        let known = match collection {
            Collection::Records => RecordKind::parse(kind.as_str()).is_some(),
            Collection::Events => crate::model::EventType::parse(kind.as_str()).is_some(),
            Collection::Evidence => {
                serde_json::from_value::<crate::model::EvidenceKind>(json!(kind)).is_ok()
            }
        };
        if !known {
            return Err(invalid(format!(
                "unknown kind {kind:?} for collection {}",
                enum_text(&collection)
            )));
        }
    }
    let newest = input.order.unwrap_or_default() == Order::Newest;
    let cursor = usize::try_from(input.cursor.unwrap_or(0)).unwrap_or(usize::MAX);
    let limit = usize::try_from(input.limit.unwrap_or(25).clamp(1, 100)).unwrap_or(25);
    let mut out = QueryOutput {
        namespace: state.namespace.as_str().to_owned(),
        snapshot_revision: state.revision,
        collection,
        records: Vec::new(),
        events: Vec::new(),
        evidence: Vec::new(),
        next_cursor: None,
        total: 0,
    };
    let page = |total: usize| -> (usize, usize, Option<u64>) {
        let start = cursor.min(total);
        let end = start.saturating_add(limit).min(total);
        let next = (end < total).then(|| u64::try_from(end).unwrap_or(u64::MAX));
        (start, end, next)
    };
    match collection {
        Collection::Records => {
            let mut matches: Vec<&Record> = state
                .records
                .values()
                .filter(|r| {
                    include_superseded || state.record_superseded_by(r.id.as_str()).is_none()
                })
                .filter(|r| id_ok(r.id.as_str()) && kind_ok(r.kind.as_str()))
                .filter(|r| status_ok(&state.record_status(r)))
                .filter(|r| {
                    category_ok(record_category(state, r)) && brand_ok(record_brand(state, r))
                })
                .filter(|r| in_range(&record_date(r), input.from.as_ref(), input.to.as_ref()))
                .collect();
            matches.sort_by(|a, b| {
                (a.ledger_revision, a.id.as_str()).cmp(&(b.ledger_revision, b.id.as_str()))
            });
            if newest {
                matches.reverse();
            }
            let items: Vec<RecordItem> = if input.include_history.unwrap_or(false) {
                matches
                    .iter()
                    .flat_map(|record| {
                        state
                            .history
                            .get(record.id.as_str())
                            .into_iter()
                            .flatten()
                            .map(|row| revision_item(state, record, row))
                    })
                    .collect::<Result<_, _>>()?
            } else {
                matches.iter().map(|r| record_item(state, r)).collect()
            };
            let (start, end, next) = page(items.len());
            out.total = u64::try_from(items.len()).unwrap_or(u64::MAX);
            out.next_cursor = next;
            out.records = items.into_iter().skip(start).take(end - start).collect();
        }
        Collection::Events => {
            let mut matches: Vec<&Event> = state
                .events
                .iter()
                .filter(|e| {
                    include_superseded || state.event_superseded_by(e.id.as_str()).is_none()
                })
                .filter(|e| id_ok(e.id.as_str()) && kind_ok(e.event_type.as_str()))
                .filter(|e| {
                    let status = if state.event_superseded_by(e.id.as_str()).is_some() {
                        "superseded"
                    } else {
                        "current"
                    };
                    status_ok(status)
                })
                .filter(|e| {
                    let variant = event_subject_variant(state, e);
                    category_ok(variant.map(|v| v.category.as_str().to_owned()))
                        && brand_ok(variant.map(|v| v.brand.clone()))
                })
                .filter(|e| in_range(&e.occurred_on, input.from.as_ref(), input.to.as_ref()))
                .collect();
            if newest {
                matches.reverse();
            }
            let (start, end, next) = page(matches.len());
            out.total = u64::try_from(matches.len()).unwrap_or(u64::MAX);
            out.next_cursor = next;
            out.events = matches[start..end]
                .iter()
                .map(|e| event_item(state, e))
                .collect();
        }
        Collection::Evidence => {
            let mut matches: Vec<&Evidence> = state
                .evidence
                .values()
                .filter(|e| id_ok(e.id.as_str()) && kind_ok(&enum_text(&e.info.kind)))
                .filter(|_| status_ok("current"))
                .filter(|e| {
                    if input.category.is_none()
                        && input.category_prefix.is_none()
                        && brand.is_none()
                    {
                        return true;
                    }
                    e.info.links.iter().any(|id| {
                        state.record(id.as_str()).is_some_and(|r| {
                            category_ok(record_category(state, r))
                                && brand_ok(record_brand(state, r))
                        })
                    })
                })
                .filter(|e| {
                    let date = e
                        .info
                        .captured_on
                        .clone()
                        .unwrap_or_else(|| iso(e.recorded_at).chars().take(10).collect());
                    in_range(&date, input.from.as_ref(), input.to.as_ref())
                })
                .collect();
            matches.sort_by(|a, b| {
                (a.ledger_revision, a.id.as_str()).cmp(&(b.ledger_revision, b.id.as_str()))
            });
            if newest {
                matches.reverse();
            }
            let (start, end, next) = page(matches.len());
            out.total = u64::try_from(matches.len()).unwrap_or(u64::MAX);
            out.next_cursor = next;
            out.evidence = matches[start..end]
                .iter()
                .map(|e| evidence_item(e))
                .collect();
        }
    }
    Ok(out)
}

/// The current measurements an id stands for.
fn measurements_for<'a>(
    state: &'a LedgerState,
    id: &str,
) -> Result<Vec<(&'a Record, &'a MeasurementRecord)>, StylistError> {
    let current = state.current_records().filter_map(|r| match &r.body {
        RecordBody::Measurement(m) => Some((r, m)),
        _ => None,
    });
    if id == BODY {
        return Ok(current
            .filter(|(_, m)| m.subject.kind == SubjectKind::Body)
            .collect());
    }
    match state.record(id).map(|r| (r, &r.body)) {
        Some((record, RecordBody::Measurement(m))) => Ok(vec![(record, m)]),
        Some((_, RecordBody::Piece(_) | RecordBody::Variant(_))) => Ok(current
            .filter(|(_, m)| m.subject.id.as_ref().is_some_and(|s| s.as_str() == id))
            .collect()),
        Some((record, _)) => Err(invalid(format!(
            "{id} is a {}; compare measurements, pieces, variants or body",
            record.kind.as_str()
        ))),
        None => Err(not_found(format!(
            "no measurement, piece or variant {id} in namespace {}",
            state.namespace
        ))),
    }
}

/// The newest measurement per (dimension, method) among `list`.
fn newest_per_dimension<'a>(
    list: Vec<(&'a Record, &'a MeasurementRecord)>,
) -> Vec<(&'a Record, &'a MeasurementRecord)> {
    let mut best: std::collections::BTreeMap<(String, String), (&Record, &MeasurementRecord)> =
        std::collections::BTreeMap::new();
    for (record, m) in list {
        let key = (m.dimension.as_str().to_owned(), enum_text(&m.method));
        let newer = best.get(&key).is_none_or(|(r, old)| {
            (m.observed_on.as_str(), record.ledger_revision)
                > (old.observed_on.as_str(), r.ledger_revision)
        });
        if newer {
            best.insert(key, (record, m));
        }
    }
    best.into_values().collect()
}

pub fn compare_state(
    state: &LedgerState,
    input: &CompareInput,
) -> Result<CompareOutput, StylistError> {
    let collect = |ids: &[String]| -> Result<Vec<(&Record, &MeasurementRecord)>, StylistError> {
        let mut all = Vec::new();
        for id in ids {
            let found = measurements_for(state, id)?;
            // A whole piece, variant or the body contributes its newest
            // measurement per dimension and method.
            if state
                .record(id)
                .is_some_and(|r| r.kind == RecordKind::Measurement)
            {
                all.extend(found);
            } else {
                all.extend(newest_per_dimension(found));
            }
        }
        if let Some(dimension) = &input.dimension {
            all.retain(|(_, m)| m.dimension == *dimension);
        }
        Ok(all)
    };
    let references = collect(&input.reference_ids)?;
    let candidates = collect(&input.candidate_ids)?;
    let mut out = CompareOutput {
        namespace: state.namespace.as_str().to_owned(),
        snapshot_revision: state.revision,
        comparable: Vec::new(),
        incompatible: Vec::new(),
        unmatched: Vec::new(),
    };
    for (ref_record, reference) in &references {
        for (cand_record, candidate) in &candidates {
            if reference.dimension != candidate.dimension || ref_record.id == cand_record.id {
                continue;
            }
            let pair = (
                measurement_ref(ref_record, reference),
                measurement_ref(cand_record, candidate),
            );
            match units::compare(reference, candidate) {
                Comparison::Comparable {
                    delta_mm,
                    tolerance_mm,
                    caveats,
                } => out.comparable.push(ComparedPair {
                    reference: pair.0,
                    candidate: pair.1,
                    delta_mm,
                    tolerance_mm,
                    caveats: caveats.iter().map(|c| c.explain().to_owned()).collect(),
                }),
                Comparison::Incompatible(reason) => out.incompatible.push(IncompatiblePair {
                    reference: pair.0,
                    candidate: pair.1,
                    reason: enum_text(&reason),
                    explanation: reason.explain().to_owned(),
                }),
            }
        }
    }
    let ref_dims: std::collections::BTreeSet<&str> = references
        .iter()
        .map(|(_, m)| m.dimension.as_str())
        .collect();
    let cand_dims: std::collections::BTreeSet<&str> = candidates
        .iter()
        .map(|(_, m)| m.dimension.as_str())
        .collect();
    out.unmatched = ref_dims
        .symmetric_difference(&cand_dims)
        .map(|d| (*d).to_owned())
        .collect();
    Ok(out)
}
