//! A namespace's ledger as of one revision, decoded into typed values, with
//! the derivations every reader shares: supersession, piece lifecycle status
//! and relationships. A single person's ledger is small, so readers and
//! write validation load the whole namespace and work in memory.

use std::collections::{BTreeMap, HashMap};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{StylistError, not_found, reject};
use crate::ledger::{self, EventRow, EvidenceRow, RecordRow};
use crate::model::{
    AcquiredEvent, AcquiredStatus, EventBody, EventType, EvidenceInfo, InitialStatus, KeepDecision,
    PieceRecord, Provenance, RecordBody, RecordKind, VariantRecord,
};
use crate::types::{EventId, EvidenceId, Namespace, RecordId};

/// What `stylist_event_append` stores for one event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub occurred_on: String,
    pub event: EventBody,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    pub provenance: Option<Provenance>,
    pub notes: Option<String>,
    pub supersedes_event_id: Option<EventId>,
    pub correction_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Record {
    pub id: RecordId,
    pub kind: RecordKind,
    pub revision: u64,
    pub ledger_revision: u64,
    pub recorded_at: i64,
    pub created_revision: u64,
    pub created_at: i64,
    pub supersedes: Option<RecordId>,
    pub correction_reason: Option<String>,
    pub body: RecordBody,
    pub payload: Value,
    pub derived: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub id: EventId,
    pub event_type: EventType,
    pub ledger_revision: u64,
    pub recorded_at: i64,
    pub occurred_on: String,
    pub envelope: EventEnvelope,
    pub payload: Value,
    pub derived: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct Evidence {
    pub id: EvidenceId,
    pub sha256: String,
    pub mime_type: String,
    pub size: u64,
    pub ledger_revision: u64,
    pub recorded_at: i64,
    pub info: EvidenceInfo,
    pub payload: Value,
}

/// A piece's lifecycle status, derived from its initial status and its
/// current (non-superseded) lifecycle events in ledger order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PieceStatus {
    Owned,
    Undecided,
    ReturnPending,
    Returned,
    Retired,
}

impl PieceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PieceStatus::Owned => "owned",
            PieceStatus::Undecided => "undecided",
            PieceStatus::ReturnPending => "return_pending",
            PieceStatus::Returned => "returned",
            PieceStatus::Retired => "retired",
        }
    }

    /// Still in hand and possibly returnable.
    pub fn is_open_decision(self) -> bool {
        matches!(self, PieceStatus::Undecided | PieceStatus::ReturnPending)
    }
}

/// A relationship from a record, event or evidence to another ledger id.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub rel: String,
    pub id: String,
}

fn link(rel: &str, id: &str) -> Link {
    Link {
        rel: rel.to_owned(),
        id: id.to_owned(),
    }
}

pub struct LedgerState {
    pub namespace: Namespace,
    /// The revision this state reflects.
    pub revision: u64,
    /// The namespace's newest revision.
    pub current_revision: u64,
    pub records: BTreeMap<String, Record>,
    /// Every revision of each record at or below `revision`, oldest first.
    pub history: BTreeMap<String, Vec<RecordRow>>,
    pub events: Vec<Event>,
    pub evidence: BTreeMap<String, Evidence>,
    record_superseded_by: HashMap<String, String>,
    event_superseded_by: HashMap<String, String>,
    event_index: HashMap<String, usize>,
}

fn corrupt(what: &str, id: &str, error: impl std::fmt::Display) -> StylistError {
    StylistError::Corrupt(format!("{what} {id}: {error}"))
}

impl LedgerState {
    /// Loads `namespace` as of `as_of` (default: its newest revision).
    pub fn load(
        conn: &Connection,
        namespace: &Namespace,
        as_of: Option<u64>,
    ) -> Result<Self, StylistError> {
        let current_revision = ledger::namespace_revision(conn, namespace.as_str())?;
        let revision = match as_of {
            Some(as_of) if as_of > current_revision => {
                return Err(reject(
                    "validation",
                    format!(
                        "asOfRevision {as_of} is after the namespace's newest revision {current_revision}"
                    ),
                ));
            }
            Some(as_of) => as_of,
            None => current_revision,
        };
        let records = ledger::record_rows(conn, namespace.as_str(), revision)?;
        let events = ledger::event_rows(conn, namespace.as_str(), revision)?;
        let evidence = ledger::evidence_rows(conn, namespace.as_str(), revision)?;
        Self::from_rows(
            namespace.clone(),
            revision,
            current_revision,
            records,
            events,
            evidence,
        )
    }

    /// Builds the state from stored rows (records ordered by id then revision).
    pub fn from_rows(
        namespace: Namespace,
        revision: u64,
        current_revision: u64,
        record_rows: Vec<RecordRow>,
        event_rows: Vec<EventRow>,
        evidence_rows: Vec<EvidenceRow>,
    ) -> Result<Self, StylistError> {
        let mut history: BTreeMap<String, Vec<RecordRow>> = BTreeMap::new();
        for row in record_rows {
            history.entry(row.id.clone()).or_default().push(row);
        }
        let mut records = BTreeMap::new();
        let mut record_superseded_by = HashMap::new();
        for (id, revisions) in &mut history {
            revisions.sort_by_key(|row| row.revision);
            let (Some(first), Some(latest)) = (revisions.first(), revisions.last()) else {
                continue;
            };
            let record = decode_record(latest, first)?;
            if let Some(old) = &record.supersedes {
                record_superseded_by.insert(old.as_str().to_owned(), id.clone());
            }
            records.insert(id.clone(), record);
        }
        let mut events = Vec::with_capacity(event_rows.len());
        let mut event_superseded_by = HashMap::new();
        let mut event_index = HashMap::new();
        for row in &event_rows {
            let event = decode_event(row)?;
            if let Some(old) = &event.envelope.supersedes_event_id {
                event_superseded_by.insert(old.as_str().to_owned(), row.id.clone());
            }
            event_index.insert(row.id.clone(), events.len());
            events.push(event);
        }
        let mut evidence = BTreeMap::new();
        for row in &evidence_rows {
            let payload: Value =
                serde_json::from_str(&row.payload).map_err(|e| corrupt("evidence", &row.id, e))?;
            let info: EvidenceInfo = serde_json::from_value(payload.clone())
                .map_err(|e| corrupt("evidence", &row.id, e))?;
            evidence.insert(
                row.id.clone(),
                Evidence {
                    id: EvidenceId(row.id.clone()),
                    sha256: row.sha256.clone(),
                    mime_type: row.mime_type.clone(),
                    size: row.size,
                    ledger_revision: row.ledger_revision,
                    recorded_at: row.recorded_at,
                    info,
                    payload,
                },
            );
        }
        Ok(Self {
            namespace,
            revision,
            current_revision,
            records,
            history,
            events,
            evidence,
            record_superseded_by,
            event_superseded_by,
            event_index,
        })
    }

    pub fn record(&self, id: &str) -> Option<&Record> {
        self.records.get(id)
    }

    /// When the newest row in this state was written.
    pub fn latest_recorded_at(&self) -> Option<i64> {
        let records = self.history.values().flatten().map(|row| row.recorded_at);
        let events = self.events.iter().map(|e| e.recorded_at);
        let evidence = self.evidence.values().map(|e| e.recorded_at);
        records.chain(events).chain(evidence).max()
    }

    /// The record `id`, which must be of `kind`.
    pub fn expect(&self, id: &str, kind: RecordKind) -> Result<&Record, StylistError> {
        match self.records.get(id) {
            Some(record) if record.kind == kind => Ok(record),
            Some(record) => Err(reject(
                "validation",
                format!(
                    "{id} is a {}, not a {}",
                    record.kind.as_str(),
                    kind.as_str()
                ),
            )),
            None => Err(not_found(format!(
                "no {} {id} in namespace {}",
                kind.as_str(),
                self.namespace
            ))),
        }
    }

    pub fn event(&self, id: &str) -> Option<&Event> {
        self.event_index.get(id).and_then(|&i| self.events.get(i))
    }

    pub fn has_id(&self, id: &str) -> bool {
        self.records.contains_key(id)
            || self.event_index.contains_key(id)
            || self.evidence.contains_key(id)
    }

    pub fn record_superseded_by(&self, id: &str) -> Option<&str> {
        self.record_superseded_by.get(id).map(String::as_str)
    }

    pub fn event_superseded_by(&self, id: &str) -> Option<&str> {
        self.event_superseded_by.get(id).map(String::as_str)
    }

    /// Events not superseded by a correction, in ledger order.
    pub fn current_events(&self) -> impl Iterator<Item = &Event> {
        self.events
            .iter()
            .filter(|event| !self.event_superseded_by.contains_key(event.id.as_str()))
    }

    /// Records not superseded by a correcting record.
    pub fn current_records(&self) -> impl Iterator<Item = &Record> {
        self.records
            .values()
            .filter(|record| !self.record_superseded_by.contains_key(record.id.as_str()))
    }

    pub fn piece(&self, id: &str) -> Option<&PieceRecord> {
        match self.records.get(id).map(|r| &r.body) {
            Some(RecordBody::Piece(piece)) => Some(piece),
            _ => None,
        }
    }

    pub fn variant(&self, id: &str) -> Option<&VariantRecord> {
        match self.records.get(id).map(|r| &r.body) {
            Some(RecordBody::Variant(variant)) => Some(variant),
            _ => None,
        }
    }

    /// The variant of a piece, when it names one.
    pub fn piece_variant(&self, piece_id: &str) -> Option<(&str, &VariantRecord)> {
        let variant_id = self.piece(piece_id)?.variant_id.as_ref()?;
        self.variant(variant_id.as_str())
            .map(|variant| (variant_id.as_str(), variant))
    }

    /// The variant behind a piece or prediction id.
    pub fn variant_of(&self, id: &str) -> Option<(&str, &VariantRecord)> {
        let record = self.records.get(id)?;
        match &record.body {
            RecordBody::Piece(_) => self.piece_variant(record.id.as_str()),
            RecordBody::Prediction(prediction) => self
                .variant(prediction.variant_id.as_str())
                .map(|variant| (prediction.variant_id.as_str(), variant)),
            RecordBody::Variant(variant) => Some((record.id.as_str(), variant)),
            _ => None,
        }
    }

    /// Current events about `piece_id`, in ledger order.
    pub fn piece_events(&self, piece_id: &str) -> impl Iterator<Item = &Event> + '_ {
        let piece_id = piece_id.to_owned();
        self.current_events().filter(move |event| {
            event
                .envelope
                .event
                .piece_id()
                .is_some_and(|id| id.as_str() == piece_id)
        })
    }

    pub fn piece_status(&self, piece_id: &str) -> PieceStatus {
        let initial = match self.piece(piece_id).and_then(|p| p.initial_status) {
            Some(InitialStatus::Undecided) => PieceStatus::Undecided,
            _ => PieceStatus::Owned,
        };
        // Backfilled history is folded in date order, ledger order breaking ties.
        let mut lifecycle: Vec<&Event> = self.piece_events(piece_id).collect();
        lifecycle.sort_by(|a, b| {
            (a.occurred_on.as_str(), a.ledger_revision)
                .cmp(&(b.occurred_on.as_str(), b.ledger_revision))
        });
        lifecycle
            .into_iter()
            .fold(initial, |status, event| match &event.envelope.event {
                EventBody::Acquired(acquired) => match acquired.status_after {
                    AcquiredStatus::Owned => PieceStatus::Owned,
                    AcquiredStatus::Undecided => PieceStatus::Undecided,
                },
                EventBody::Decision(decision) => match decision.decision {
                    KeepDecision::Keep => PieceStatus::Owned,
                    KeepDecision::Return => PieceStatus::ReturnPending,
                    KeepDecision::Undecided => PieceStatus::Undecided,
                },
                EventBody::Returned(_) => PieceStatus::Returned,
                EventBody::Retired(_) => PieceStatus::Retired,
                _ => status,
            })
    }

    /// The latest current acquisition of a piece.
    pub fn acquisition(&self, piece_id: &str) -> Option<(&Event, &AcquiredEvent)> {
        self.piece_events(piece_id)
            .filter_map(|event| match &event.envelope.event {
                EventBody::Acquired(acquired) => Some((event, acquired)),
                _ => None,
            })
            .last()
    }

    /// The record's status as served: superseded, a piece's lifecycle status,
    /// a rule's or need's own status, otherwise `current`.
    pub fn record_status(&self, record: &Record) -> String {
        if self.record_superseded_by(record.id.as_str()).is_some() {
            return "superseded".to_owned();
        }
        match &record.body {
            RecordBody::Piece(_) => self.piece_status(record.id.as_str()).as_str().to_owned(),
            RecordBody::Rule(rule) => enum_text(&rule.status),
            RecordBody::Need(need) => enum_text(&need.status),
            _ => "current".to_owned(),
        }
    }

    /// The ledger ids a record refers to.
    pub fn record_links(&self, record: &Record) -> Vec<Link> {
        let mut links = Vec::new();
        let evidence = |links: &mut Vec<Link>, provenance: &Provenance| {
            for id in &provenance.evidence_ids {
                links.push(link("evidence", id.as_str()));
            }
        };
        match &record.body {
            RecordBody::Variant(v) => evidence(&mut links, &v.provenance),
            RecordBody::Piece(p) => {
                if let Some(id) = &p.variant_id {
                    links.push(link("variant", id.as_str()));
                }
                evidence(&mut links, &p.provenance);
            }
            RecordBody::Measurement(m) => {
                if let Some(id) = &m.subject.id {
                    links.push(link("subject", id.as_str()));
                }
                if let Some(id) = m
                    .garment_state
                    .as_ref()
                    .and_then(|s| s.after_event_id.as_ref())
                {
                    links.push(link("after_event", id.as_str()));
                }
                evidence(&mut links, &m.provenance);
            }
            RecordBody::ProfileSnapshot(p) => evidence(&mut links, &p.provenance),
            RecordBody::Preference(p) => evidence(&mut links, &p.provenance),
            RecordBody::Prediction(p) => {
                links.push(link("variant", p.variant_id.as_str()));
                for id in &p.references_used {
                    links.push(link("reference", id.as_str()));
                }
                for id in &p.rules_applied {
                    links.push(link("rule", id.as_str()));
                }
                if let Some(id) = &p.need_id {
                    links.push(link("need", id.as_str()));
                }
                for expectation in &p.region_expectations {
                    for id in &expectation.evidence_refs {
                        links.push(link("expectation_evidence", id.as_str()));
                    }
                }
            }
            RecordBody::Rule(r) => {
                for id in &r.evidence {
                    links.push(link("evidence", id.as_str()));
                }
                for case in &r.regression {
                    links.push(link("regression_case", case.case_id.as_str()));
                }
                if let Some(id) = &r.supersedes_rule_id {
                    links.push(link("supersedes_rule", id.as_str()));
                }
            }
            RecordBody::Need(n) => {
                for id in &n.related_piece_ids {
                    links.push(link("related_piece", id.as_str()));
                }
                evidence(&mut links, &n.provenance);
            }
        }
        if let Some(id) = &record.supersedes {
            links.push(link("supersedes", id.as_str()));
        }
        links.sort();
        links.dedup();
        links
    }

    /// The ledger ids an event refers to.
    pub fn event_links(&self, event: &Event) -> Vec<Link> {
        let mut links = Vec::new();
        let envelope = &event.envelope;
        if let Some(id) = envelope.event.piece_id() {
            links.push(link("piece", id.as_str()));
        }
        if let Some(id) = envelope.event.prediction_id() {
            links.push(link("prediction", id.as_str()));
        }
        match &envelope.event {
            EventBody::Wash(wash) => {
                for id in &wash.measurement_ids {
                    links.push(link("measurement", id.as_str()));
                }
            }
            EventBody::Outcome(outcome) => {
                for id in &outcome.based_on_event_ids {
                    links.push(link("based_on", id.as_str()));
                }
            }
            EventBody::Contradiction(c) => {
                for id in &c.ids {
                    links.push(link("concerns", id.as_str()));
                }
                if let Some(id) = &c.resolves_event_id {
                    links.push(link("resolves", id.as_str()));
                }
            }
            EventBody::Note(note) => {
                for id in &note.ids {
                    links.push(link("concerns", id.as_str()));
                }
            }
            _ => {}
        }
        for id in &envelope.evidence_ids {
            links.push(link("evidence", id.as_str()));
        }
        if let Some(provenance) = &envelope.provenance {
            for id in &provenance.evidence_ids {
                links.push(link("evidence", id.as_str()));
            }
        }
        if let Some(id) = &envelope.supersedes_event_id {
            links.push(link("supersedes", id.as_str()));
        }
        links.sort();
        links.dedup();
        links
    }

    pub fn evidence_links(evidence: &Evidence) -> Vec<Link> {
        evidence
            .info
            .links
            .iter()
            .map(|id| link("supports", id.as_str()))
            .collect()
    }

    /// Records, events and evidence linking to `id`, plus what `id` links to.
    pub fn related_ids(&self, id: &str) -> std::collections::BTreeSet<String> {
        let mut related = std::collections::BTreeSet::new();
        for record in self.records.values() {
            let links = self.record_links(record);
            if record.id.as_str() == id {
                related.extend(links.into_iter().map(|l| l.id));
            } else if links.iter().any(|l| l.id == id) {
                related.insert(record.id.as_str().to_owned());
            }
        }
        for event in &self.events {
            let links = self.event_links(event);
            if event.id.as_str() == id {
                related.extend(links.into_iter().map(|l| l.id));
            } else if links.iter().any(|l| l.id == id) {
                related.insert(event.id.as_str().to_owned());
            }
        }
        for evidence in self.evidence.values() {
            let links = Self::evidence_links(evidence);
            if evidence.id.as_str() == id {
                related.extend(links.into_iter().map(|l| l.id));
            } else if links.iter().any(|l| l.id == id) {
                related.insert(evidence.id.as_str().to_owned());
            }
        }
        related
    }
}

/// The snake_case name of a serde enum value.
pub fn enum_text<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(text)) => text,
        _ => String::new(),
    }
}

fn decode_record(latest: &RecordRow, first: &RecordRow) -> Result<Record, StylistError> {
    let payload: Value =
        serde_json::from_str(&latest.payload).map_err(|e| corrupt("record", &latest.id, e))?;
    let body: RecordBody =
        serde_json::from_value(payload.clone()).map_err(|e| corrupt("record", &latest.id, e))?;
    let kind = RecordKind::parse(&latest.kind).ok_or_else(|| {
        corrupt(
            "record",
            &latest.id,
            format!("unknown kind {}", latest.kind),
        )
    })?;
    if body.kind() != kind {
        return Err(corrupt(
            "record",
            &latest.id,
            "kind column disagrees with payload",
        ));
    }
    let derived = latest
        .derived
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| corrupt("record", &latest.id, e))?;
    Ok(Record {
        id: RecordId(latest.id.clone()),
        kind,
        revision: latest.revision,
        ledger_revision: latest.ledger_revision,
        recorded_at: latest.recorded_at,
        created_revision: first.ledger_revision,
        created_at: first.recorded_at,
        supersedes: latest.supersedes.clone().map(RecordId),
        correction_reason: latest.correction_reason.clone(),
        body,
        payload,
        derived,
    })
}

fn decode_event(row: &EventRow) -> Result<Event, StylistError> {
    let payload: Value =
        serde_json::from_str(&row.payload).map_err(|e| corrupt("event", &row.id, e))?;
    let envelope: EventEnvelope =
        serde_json::from_value(payload.clone()).map_err(|e| corrupt("event", &row.id, e))?;
    let event_type = EventType::parse(&row.event_type)
        .ok_or_else(|| corrupt("event", &row.id, format!("unknown type {}", row.event_type)))?;
    if envelope.event.event_type() != event_type {
        return Err(corrupt(
            "event",
            &row.id,
            "type column disagrees with payload",
        ));
    }
    let derived = row
        .derived
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| corrupt("event", &row.id, e))?;
    Ok(Event {
        id: EventId(row.id.clone()),
        event_type,
        ledger_revision: row.ledger_revision,
        recorded_at: row.recorded_at,
        occurred_on: row.occurred_on.clone(),
        envelope,
        payload,
        derived,
    })
}
