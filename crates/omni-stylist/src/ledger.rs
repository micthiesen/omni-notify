//! The ledger tables beside `blobs` in the docstore. Every write runs in one
//! `BEGIN IMMEDIATE` transaction: idempotency lookup, validation against the
//! loaded state, the new rows, the namespace revision bump and the idempotency
//! receipt commit together or not at all.
//!
//! - `stylist_namespaces`: one row per namespace with its ledger revision,
//!   which increases by one on every committed write.
//! - `stylist_records`: every revision of every record; the newest revision at
//!   or below a ledger revision is that record's state at that revision.
//! - `stylist_events`: immutable events in ledger order.
//! - `stylist_evidence`: evidence metadata; bytes live in the private
//!   content-addressed directory ([`crate::evidence`]).
//! - `stylist_idempotency`: the result of every keyed write.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{StylistError, sql};

pub const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS stylist_namespaces (
       namespace TEXT PRIMARY KEY,
       revision INTEGER NOT NULL,
       created_at INTEGER NOT NULL,
       updated_at INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS stylist_records (
       namespace TEXT NOT NULL,
       id TEXT NOT NULL,
       revision INTEGER NOT NULL,
       kind TEXT NOT NULL,
       ledger_revision INTEGER NOT NULL,
       recorded_at INTEGER NOT NULL,
       supersedes TEXT,
       correction_reason TEXT,
       payload TEXT NOT NULL,
       derived TEXT,
       PRIMARY KEY (namespace, id, revision))",
    "CREATE INDEX IF NOT EXISTS stylist_records_by_revision
       ON stylist_records (namespace, ledger_revision)",
    "CREATE TABLE IF NOT EXISTS stylist_events (
       namespace TEXT NOT NULL,
       id TEXT NOT NULL,
       type TEXT NOT NULL,
       ledger_revision INTEGER NOT NULL,
       occurred_on TEXT NOT NULL,
       recorded_at INTEGER NOT NULL,
       supersedes TEXT,
       payload TEXT NOT NULL,
       derived TEXT,
       PRIMARY KEY (namespace, id))",
    "CREATE INDEX IF NOT EXISTS stylist_events_by_revision
       ON stylist_events (namespace, ledger_revision)",
    "CREATE TABLE IF NOT EXISTS stylist_evidence (
       namespace TEXT NOT NULL,
       id TEXT NOT NULL,
       sha256 TEXT NOT NULL,
       mime_type TEXT NOT NULL,
       size INTEGER NOT NULL,
       ledger_revision INTEGER NOT NULL,
       recorded_at INTEGER NOT NULL,
       payload TEXT NOT NULL,
       PRIMARY KEY (namespace, id),
       UNIQUE (namespace, sha256))",
    "CREATE TABLE IF NOT EXISTS stylist_idempotency (
       namespace TEXT NOT NULL,
       key TEXT NOT NULL,
       tool TEXT NOT NULL,
       fingerprint TEXT NOT NULL,
       ledger_revision INTEGER NOT NULL,
       recorded_at INTEGER NOT NULL,
       result TEXT NOT NULL,
       PRIMARY KEY (namespace, key))",
    // One correction per original.
    "CREATE UNIQUE INDEX IF NOT EXISTS stylist_records_one_correction
       ON stylist_records (namespace, supersedes) WHERE supersedes IS NOT NULL",
    "CREATE UNIQUE INDEX IF NOT EXISTS stylist_events_one_correction
       ON stylist_events (namespace, supersedes) WHERE supersedes IS NOT NULL",
    // Ledger rows are append-only, and the real namespace's rows are never deleted.
    "CREATE TRIGGER IF NOT EXISTS stylist_records_no_update BEFORE UPDATE ON stylist_records
       BEGIN SELECT RAISE(ABORT, 'stylist ledger rows are append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_events_no_update BEFORE UPDATE ON stylist_events
       BEGIN SELECT RAISE(ABORT, 'stylist ledger rows are append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_evidence_no_update BEFORE UPDATE ON stylist_evidence
       BEGIN SELECT RAISE(ABORT, 'stylist ledger rows are append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_idempotency_no_update BEFORE UPDATE ON stylist_idempotency
       BEGIN SELECT RAISE(ABORT, 'stylist ledger rows are append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_records_keep_real BEFORE DELETE ON stylist_records
       WHEN OLD.namespace = 'real'
       BEGIN SELECT RAISE(ABORT, 'the real stylist ledger is never deleted'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_events_keep_real BEFORE DELETE ON stylist_events
       WHEN OLD.namespace = 'real'
       BEGIN SELECT RAISE(ABORT, 'the real stylist ledger is never deleted'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_evidence_keep_real BEFORE DELETE ON stylist_evidence
       WHEN OLD.namespace = 'real'
       BEGIN SELECT RAISE(ABORT, 'the real stylist ledger is never deleted'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_idempotency_keep_real BEFORE DELETE ON stylist_idempotency
       WHEN OLD.namespace = 'real'
       BEGIN SELECT RAISE(ABORT, 'the real stylist ledger is never deleted'); END",
    "CREATE TRIGGER IF NOT EXISTS stylist_namespaces_keep_real BEFORE DELETE ON stylist_namespaces
       WHEN OLD.namespace = 'real'
       BEGIN SELECT RAISE(ABORT, 'the real stylist ledger is never deleted'); END",
];

/// Creates the tables and indexes (idempotent).
pub fn ensure_schema(conn: &Connection) -> Result<(), StylistError> {
    for statement in DDL {
        conn.execute_batch(statement).map_err(sql)?;
    }
    Ok(())
}

/// One stored record revision, as exported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordRow {
    pub id: String,
    pub revision: u64,
    pub kind: String,
    pub ledger_revision: u64,
    pub recorded_at: i64,
    pub supersedes: Option<String>,
    pub correction_reason: Option<String>,
    pub payload: String,
    pub derived: Option<String>,
}

/// One stored event, as exported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub ledger_revision: u64,
    pub occurred_on: String,
    pub recorded_at: i64,
    pub supersedes: Option<String>,
    pub payload: String,
    pub derived: Option<String>,
}

/// One stored evidence row, as exported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRow {
    pub id: String,
    pub sha256: String,
    pub mime_type: String,
    pub size: u64,
    pub ledger_revision: u64,
    pub recorded_at: i64,
    pub payload: String,
}

/// One idempotency receipt, as exported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdempotencyRow {
    pub key: String,
    pub tool: String,
    pub fingerprint: String,
    pub ledger_revision: u64,
    pub recorded_at: i64,
    pub result: String,
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The namespace's current ledger revision (0 when it has no writes).
pub fn namespace_revision(conn: &Connection, namespace: &str) -> Result<u64, StylistError> {
    conn.prepare_cached("SELECT revision FROM stylist_namespaces WHERE namespace = ?1")
        .and_then(|mut stmt| {
            stmt.query_row(params![namespace], |row| row.get::<_, i64>(0))
                .optional()
        })
        .map(|revision| revision.map_or(0, to_u64))
        .map_err(sql)
}

/// Every namespace with its revision.
pub fn namespaces(conn: &Connection) -> Result<Vec<(String, u64)>, StylistError> {
    conn.prepare_cached("SELECT namespace, revision FROM stylist_namespaces ORDER BY namespace")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, to_u64(row.get::<_, i64>(1)?)))
            })?
            .collect()
        })
        .map_err(sql)
}

/// Advances the namespace to its next revision and returns it.
pub fn next_revision(conn: &Connection, namespace: &str, now: i64) -> Result<u64, StylistError> {
    let next = namespace_revision(conn, namespace)? + 1;
    set_revision(conn, namespace, next, now)?;
    Ok(next)
}

/// Sets the namespace revision (restore).
pub fn set_revision(
    conn: &Connection,
    namespace: &str,
    revision: u64,
    now: i64,
) -> Result<(), StylistError> {
    conn.prepare_cached(
        "INSERT INTO stylist_namespaces (namespace, revision, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(namespace) DO UPDATE SET revision = excluded.revision,
           updated_at = excluded.updated_at",
    )
    .and_then(|mut stmt| stmt.execute(params![namespace, to_i64(revision), now]))
    .map(|_| ())
    .map_err(sql)
}

pub fn insert_record(
    conn: &Connection,
    namespace: &str,
    row: &RecordRow,
) -> Result<(), StylistError> {
    conn.prepare_cached(
        "INSERT INTO stylist_records (namespace, id, revision, kind, ledger_revision,
           recorded_at, supersedes, correction_reason, payload, derived)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .and_then(|mut stmt| {
        stmt.execute(params![
            namespace,
            row.id,
            to_i64(row.revision),
            row.kind,
            to_i64(row.ledger_revision),
            row.recorded_at,
            row.supersedes,
            row.correction_reason,
            row.payload,
            row.derived,
        ])
    })
    .map(|_| ())
    .map_err(sql)
}

pub fn insert_event(
    conn: &Connection,
    namespace: &str,
    row: &EventRow,
) -> Result<(), StylistError> {
    conn.prepare_cached(
        "INSERT INTO stylist_events (namespace, id, type, ledger_revision, occurred_on,
           recorded_at, supersedes, payload, derived)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .and_then(|mut stmt| {
        stmt.execute(params![
            namespace,
            row.id,
            row.event_type,
            to_i64(row.ledger_revision),
            row.occurred_on,
            row.recorded_at,
            row.supersedes,
            row.payload,
            row.derived,
        ])
    })
    .map(|_| ())
    .map_err(sql)
}

pub fn insert_evidence(
    conn: &Connection,
    namespace: &str,
    row: &EvidenceRow,
) -> Result<(), StylistError> {
    conn.prepare_cached(
        "INSERT INTO stylist_evidence (namespace, id, sha256, mime_type, size,
           ledger_revision, recorded_at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .and_then(|mut stmt| {
        stmt.execute(params![
            namespace,
            row.id,
            row.sha256,
            row.mime_type,
            to_i64(row.size),
            to_i64(row.ledger_revision),
            row.recorded_at,
            row.payload,
        ])
    })
    .map(|_| ())
    .map_err(sql)
}

pub fn insert_idempotency(
    conn: &Connection,
    namespace: &str,
    row: &IdempotencyRow,
) -> Result<(), StylistError> {
    conn.prepare_cached(
        "INSERT INTO stylist_idempotency (namespace, key, tool, fingerprint,
           ledger_revision, recorded_at, result)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .and_then(|mut stmt| {
        stmt.execute(params![
            namespace,
            row.key,
            row.tool,
            row.fingerprint,
            to_i64(row.ledger_revision),
            row.recorded_at,
            row.result,
        ])
    })
    .map(|_| ())
    .map_err(sql)
}

pub fn idempotency(
    conn: &Connection,
    namespace: &str,
    key: &str,
) -> Result<Option<IdempotencyRow>, StylistError> {
    conn.prepare_cached(
        "SELECT key, tool, fingerprint, ledger_revision, recorded_at, result
         FROM stylist_idempotency WHERE namespace = ?1 AND key = ?2",
    )
    .and_then(|mut stmt| {
        stmt.query_row(params![namespace, key], |row| {
            Ok(IdempotencyRow {
                key: row.get(0)?,
                tool: row.get(1)?,
                fingerprint: row.get(2)?,
                ledger_revision: to_u64(row.get(3)?),
                recorded_at: row.get(4)?,
                result: row.get(5)?,
            })
        })
        .optional()
    })
    .map_err(sql)
}

/// Every record revision at or below `as_of`, by id then revision.
pub fn record_rows(
    conn: &Connection,
    namespace: &str,
    as_of: u64,
) -> Result<Vec<RecordRow>, StylistError> {
    conn.prepare_cached(
        "SELECT id, revision, kind, ledger_revision, recorded_at, supersedes,
           correction_reason, payload, derived
         FROM stylist_records WHERE namespace = ?1 AND ledger_revision <= ?2
         ORDER BY id, revision",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![namespace, to_i64(as_of)], |row| {
            Ok(RecordRow {
                id: row.get(0)?,
                revision: to_u64(row.get(1)?),
                kind: row.get(2)?,
                ledger_revision: to_u64(row.get(3)?),
                recorded_at: row.get(4)?,
                supersedes: row.get(5)?,
                correction_reason: row.get(6)?,
                payload: row.get(7)?,
                derived: row.get(8)?,
            })
        })?
        .collect()
    })
    .map_err(sql)
}

/// Every event at or below `as_of`, in ledger order.
pub fn event_rows(
    conn: &Connection,
    namespace: &str,
    as_of: u64,
) -> Result<Vec<EventRow>, StylistError> {
    conn.prepare_cached(
        "SELECT id, type, ledger_revision, occurred_on, recorded_at, supersedes, payload, derived
         FROM stylist_events WHERE namespace = ?1 AND ledger_revision <= ?2
         ORDER BY ledger_revision, id",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![namespace, to_i64(as_of)], |row| {
            Ok(EventRow {
                id: row.get(0)?,
                event_type: row.get(1)?,
                ledger_revision: to_u64(row.get(2)?),
                occurred_on: row.get(3)?,
                recorded_at: row.get(4)?,
                supersedes: row.get(5)?,
                payload: row.get(6)?,
                derived: row.get(7)?,
            })
        })?
        .collect()
    })
    .map_err(sql)
}

/// Every evidence row at or below `as_of`, in ledger order.
pub fn evidence_rows(
    conn: &Connection,
    namespace: &str,
    as_of: u64,
) -> Result<Vec<EvidenceRow>, StylistError> {
    conn.prepare_cached(
        "SELECT id, sha256, mime_type, size, ledger_revision, recorded_at, payload
         FROM stylist_evidence WHERE namespace = ?1 AND ledger_revision <= ?2
         ORDER BY ledger_revision, id",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![namespace, to_i64(as_of)], |row| {
            Ok(EvidenceRow {
                id: row.get(0)?,
                sha256: row.get(1)?,
                mime_type: row.get(2)?,
                size: to_u64(row.get(3)?),
                ledger_revision: to_u64(row.get(4)?),
                recorded_at: row.get(5)?,
                payload: row.get(6)?,
            })
        })?
        .collect()
    })
    .map_err(sql)
}

pub fn idempotency_rows(
    conn: &Connection,
    namespace: &str,
) -> Result<Vec<IdempotencyRow>, StylistError> {
    conn.prepare_cached(
        "SELECT key, tool, fingerprint, ledger_revision, recorded_at, result
         FROM stylist_idempotency WHERE namespace = ?1 ORDER BY ledger_revision, key",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![namespace], |row| {
            Ok(IdempotencyRow {
                key: row.get(0)?,
                tool: row.get(1)?,
                fingerprint: row.get(2)?,
                ledger_revision: to_u64(row.get(3)?),
                recorded_at: row.get(4)?,
                result: row.get(5)?,
            })
        })?
        .collect()
    })
    .map_err(sql)
}

/// The evidence row for `sha256` in `namespace`, if any.
pub fn evidence_by_hash(
    conn: &Connection,
    namespace: &str,
    sha256: &str,
) -> Result<Option<String>, StylistError> {
    conn.prepare_cached("SELECT id FROM stylist_evidence WHERE namespace = ?1 AND sha256 = ?2")
        .and_then(|mut stmt| {
            stmt.query_row(params![namespace, sha256], |row| row.get(0))
                .optional()
        })
        .map_err(sql)
}

/// Every distinct evidence digest in any namespace.
pub fn all_evidence_hashes(conn: &Connection) -> Result<Vec<String>, StylistError> {
    conn.prepare_cached("SELECT DISTINCT sha256 FROM stylist_evidence")
        .and_then(|mut stmt| stmt.query_map([], |row| row.get(0))?.collect())
        .map_err(sql)
}

/// Whether the namespace has no rows at all.
pub fn namespace_is_empty(conn: &Connection, namespace: &str) -> Result<bool, StylistError> {
    for table in [
        "stylist_records",
        "stylist_events",
        "stylist_evidence",
        "stylist_idempotency",
    ] {
        let found: bool = conn
            .prepare_cached(&format!(
                "SELECT 1 FROM {table} WHERE namespace = ?1 LIMIT 1"
            ))
            .and_then(|mut stmt| stmt.exists(params![namespace]))
            .map_err(sql)?;
        if found {
            return Ok(false);
        }
    }
    Ok(namespace_revision(conn, namespace)? == 0)
}

/// Deletes every row of `namespace`; returns the rows removed per table.
pub fn delete_namespace(
    conn: &Connection,
    namespace: &str,
) -> Result<Vec<(String, u64)>, StylistError> {
    let mut removed = Vec::new();
    for table in [
        "stylist_records",
        "stylist_events",
        "stylist_evidence",
        "stylist_idempotency",
        "stylist_namespaces",
    ] {
        let count = conn
            .prepare_cached(&format!("DELETE FROM {table} WHERE namespace = ?1"))
            .and_then(|mut stmt| stmt.execute(params![namespace]))
            .map_err(sql)?;
        removed.push((table.to_owned(), u64::try_from(count).unwrap_or(0)));
    }
    Ok(removed)
}
