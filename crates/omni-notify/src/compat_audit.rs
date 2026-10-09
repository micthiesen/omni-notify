//! `omni-notify compat-audit --db <copy> [--rewrite-to <new.db>]`
//! (ARCHITECTURE.md 4.5 step 3): typed per-entity compatibility of a
//! production copy.
//!
//! Per entity: rows, expired rows, CBOR decode failures, typed decode
//! failures, `recompute_pk(data) != pk` mismatches, and the typed round trip
//! `decode -> typed -> encode -> decode`, which must equal the original JS
//! value once `undefined` object fields are removed (byte-identical re-encodes
//! are reported for information). Rows whose entity no subsystem declares and
//! legacy `$` rows with a NULL entity column are listed separately.
//!
//! The source is copied into a temporary directory first; the given file is
//! never opened. `--rewrite-to` writes every row, re-encoded through its typed
//! model (other rows byte for byte), into a fresh database for
//! `cargo xtask node-readback <copy> <new.db>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use omni_store::cbor::{self, JsValue};
use omni_store::entity::EntityDescriptor;

/// Every entity the binary persists: the foundation's plus each subsystem's.
pub fn entity_catalog() -> Vec<EntityDescriptor> {
    let mut all = vec![
        EntityDescriptor::of::<omni_tasks::persistence::TaskRunData>(),
        EntityDescriptor::of::<omni_tasks::persistence::TaskRunLog>(),
        EntityDescriptor::of::<omni_tasks::persistence::TaskScheduleState>(),
        EntityDescriptor::of::<omni_ai::costs::CostEventData>(),
        EntityDescriptor::of::<omni_ai::costs::CostMigrationData>(),
        EntityDescriptor::of::<omni_ios_controls::persistence::IosControlRegistration>(),
    ];
    for group in [
        omni_live::entities(),
        omni_live_intel::entities(),
        omni_personal::entities(),
        omni_briefings::entities(),
        omni_presspods::entities(),
        omni_media::entity_descriptors(),
        omni_arr::entities(),
        omni_podcasts::entities(),
        omni_workspaces::entities(),
        omni_imap::subsystem::entities(),
        omni_parcel::entities(),
        omni_calendar::entities(),
        omni_email::entities(),
        omni_mcp::entities(),
    ] {
        for descriptor in group {
            if !all.iter().any(|d| d.name == descriptor.name) {
                all.push(descriptor);
            }
        }
    }
    all
}

/// Raw-key collections written with `upsert_doc` and an explicit entity
/// column (ARCHITECTURE.md 3.3): read as plain JS values, never typed or
/// migrated, so they are checked at the CBOR level only.
pub const RAW_COLLECTIONS: &[&str] = &[
    omni_imap::archive_store::ACTION_ENTITY,
    omni_imap::archive_store::MESSAGE_ENTITY,
    omni_imap::archive_store::HISTORY_ENTITY,
    omni_imap::compose::DRAFT_ENTITY,
    omni_imap::compose::SEND_ENTITY,
];

/// Per-entity counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityAudit {
    pub rows: u64,
    pub expired: u64,
    pub null_data: u64,
    pub cbor_failures: u64,
    pub typed_failures: u64,
    pub pk_mismatches: u64,
    /// Typed re-encode decodes to a different JS value (ignoring `undefined` fields).
    pub value_diffs: u64,
    pub byte_identical: u64,
}

/// The audit of one database.
#[derive(Clone, Debug, Default)]
pub struct Audit {
    pub entities: BTreeMap<String, EntityAudit>,
    /// Entity names present in the database that no code reads (TS
    /// leftovers such as `jmap-email-state`); informational.
    pub orphans: BTreeMap<String, u64>,
    pub legacy_null_entity: u64,
    /// Up to 20 example failures (`pk: reason`).
    pub samples: Vec<String>,
    pub rewritten: u64,
}

impl Audit {
    /// Failures that block a cutover: CBOR and typed decode failures, key
    /// mismatches and value diffs.
    pub fn blocking(&self) -> u64 {
        self.entities
            .values()
            .map(|e| e.cbor_failures + e.typed_failures + e.pk_mismatches + e.value_diffs)
            .sum::<u64>()
    }

    /// A plain-text report.
    pub fn report(&self) -> String {
        let mut out = String::from(
            "entity                              rows  expired  cbor  typed  pk  diffs  byte-identical\n",
        );
        for (name, e) in &self.entities {
            let pct = if e.rows == e.null_data {
                100.0
            } else {
                #[allow(clippy::cast_precision_loss)]
                let pct = e.byte_identical as f64 * 100.0 / (e.rows - e.null_data) as f64;
                pct
            };
            out.push_str(&format!(
                "{name:<34} {:>6} {:>8} {:>5} {:>6} {:>3} {:>6} {pct:>13.1}%\n",
                e.rows,
                e.expired,
                e.cbor_failures,
                e.typed_failures,
                e.pk_mismatches,
                e.value_diffs
            ));
        }
        for (name, rows) in &self.orphans {
            out.push_str(&format!(
                "orphan entity {name} (no reader): {rows} row(s)\n"
            ));
        }
        out.push_str(&format!(
            "legacy NULL-entity $ rows: {}\n",
            self.legacy_null_entity
        ));
        if self.rewritten > 0 {
            out.push_str(&format!("rewritten rows: {}\n", self.rewritten));
        }
        for sample in &self.samples {
            out.push_str(&format!("  {sample}\n"));
        }
        out.push_str(&format!("blocking issues: {}\n", self.blocking()));
        out
    }
}

/// Removes `undefined` object fields recursively (TS reads absent and
/// `undefined` identically).
pub fn strip_undefined(value: &JsValue) -> JsValue {
    match value {
        JsValue::Object(map) => JsValue::Object(
            map.iter()
                .filter(|(_, v)| !matches!(v, JsValue::Undefined))
                .map(|(k, v)| (k.clone(), strip_undefined(v)))
                .collect(),
        ),
        JsValue::Array(items) => JsValue::Array(items.iter().map(strip_undefined).collect()),
        JsValue::Map(entries) => JsValue::Map(
            entries
                .iter()
                .map(|(k, v)| (strip_undefined(k), strip_undefined(v)))
                .collect(),
        ),
        JsValue::Set(items) => JsValue::Set(items.iter().map(strip_undefined).collect()),
        JsValue::Tagged(tag, inner) => JsValue::Tagged(*tag, Box::new(strip_undefined(inner))),
        other => other.clone(),
    }
}

/// JS value equality where numbers compare as JS numbers (CBOR int `3` and
/// float `3.0` are the same JS value).
pub fn js_equal(a: &JsValue, b: &JsValue) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let number = |v: &JsValue| match v {
        JsValue::Int(i) => Some(*i as f64),
        JsValue::Float(f) => Some(*f),
        _ => None,
    };
    match (a, b) {
        (JsValue::Object(x), JsValue::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| js_equal(v, w)))
        }
        (JsValue::Array(x), JsValue::Array(y)) | (JsValue::Set(x), JsValue::Set(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| js_equal(v, w))
        }
        (JsValue::Map(x), JsValue::Map(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((k1, v1), (k2, v2))| js_equal(k1, k2) && js_equal(v1, v2))
        }
        (JsValue::Tagged(t1, v1), JsValue::Tagged(t2, v2)) => t1 == t2 && js_equal(v1, v2),
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y || (x.is_nan() && y.is_nan()),
            _ => a == b,
        },
    }
}

struct Row {
    pk: String,
    data: Option<Vec<u8>>,
    entity: Option<String>,
    version: rusqlite::types::Value,
    expires_at: rusqlite::types::Value,
    updated_at: rusqlite::types::Value,
}

fn integer(value: &rusqlite::types::Value) -> Option<i64> {
    match value {
        rusqlite::types::Value::Integer(i) => Some(*i),
        #[allow(clippy::cast_possible_truncation)]
        rusqlite::types::Value::Real(f) => Some(*f as i64),
        _ => None,
    }
}

fn read_rows(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<Row>> {
    let mut statement = conn.prepare(
        "SELECT pk, data, entity, version, expires_at, updated_at FROM blobs ORDER BY pk",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Row {
            pk: row.get("pk")?,
            data: row.get("data")?,
            entity: row.get("entity")?,
            version: row.get("version")?,
            expires_at: row.get("expires_at")?,
            updated_at: row.get("updated_at")?,
        })
    })?;
    rows.collect()
}

/// Audits `conn` (a copy) and, with `rewrite`, writes re-encoded rows there.
pub fn audit(
    conn: &rusqlite::Connection,
    catalog: &[EntityDescriptor],
    now_ms: i64,
    rewrite: Option<&rusqlite::Connection>,
) -> anyhow::Result<Audit> {
    let mut audit = Audit::default();
    for row in read_rows(conn)? {
        if row.entity.is_none() && row.pk.starts_with('$') {
            audit.legacy_null_entity += 1;
        }
        let mut out_bytes = row.data.clone();
        if let Some(name) = row.entity.as_deref() {
            match catalog.iter().find(|d| d.name == name) {
                None if RAW_COLLECTIONS.contains(&name) => {
                    let stats = audit.entities.entry(name.to_owned()).or_default();
                    audit_raw_row(stats, &mut audit.samples, name, &row, now_ms);
                }
                None => *audit.orphans.entry(name.to_owned()).or_default() += 1,
                Some(descriptor) => {
                    let stats = audit
                        .entities
                        .entry(descriptor.name.to_owned())
                        .or_default();
                    if let Some(bytes) =
                        audit_row(stats, &mut audit.samples, descriptor, &row, now_ms)
                    {
                        out_bytes = Some(bytes);
                    }
                }
            }
        }
        if let Some(target) = rewrite {
            target.execute(
                "INSERT INTO blobs (pk, data, entity, version, expires_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    row.pk,
                    out_bytes,
                    row.entity,
                    row.version,
                    row.expires_at,
                    row.updated_at
                ],
            )?;
            audit.rewritten += 1;
        }
    }
    Ok(audit)
}

/// Decodes a raw-collection row with the type omni-imap reads it as.
fn decode_raw(entity: &str, pk: &str, value: JsValue) -> Result<(), String> {
    use omni_imap::{archive_store, compose};
    match entity {
        archive_store::ACTION_ENTITY => archive_store::decode_action(pk, value)
            .map(drop)
            .map_err(|e| e.to_string()),
        archive_store::MESSAGE_ENTITY => cbor::from_value::<String>(value)
            .map(drop)
            .map_err(|e| e.to_string()),
        archive_store::HISTORY_ENTITY => cbor::from_value::<Vec<String>>(value)
            .map(drop)
            .map_err(|e| e.to_string()),
        compose::DRAFT_ENTITY | compose::SEND_ENTITY => {
            cbor::from_value::<compose::StoredAttempt>(value)
                .map(drop)
                .map_err(|e| e.to_string())
        }
        other => Err(format!("no reader for raw collection {other}")),
    }
}

/// Audits one row of a raw collection: CBOR decode, re-encode, and a typed
/// decode with omni-imap's reader (rows stay byte-for-byte as stored).
fn audit_raw_row(
    stats: &mut EntityAudit,
    samples: &mut Vec<String>,
    entity: &str,
    row: &Row,
    now_ms: i64,
) {
    stats.rows += 1;
    if integer(&row.expires_at).is_some_and(|at| at <= now_ms) {
        stats.expired += 1;
    }
    let Some(bytes) = &row.data else {
        stats.null_data += 1;
        return;
    };
    match cbor::decode(bytes) {
        Ok(value) => {
            if cbor::encode(&value) == *bytes {
                stats.byte_identical += 1;
            }
            if let Err(error) = decode_raw(entity, &row.pk, value) {
                stats.typed_failures += 1;
                sample(samples, &row.pk, format!("typed: {error}"));
            }
        }
        Err(error) => {
            stats.cbor_failures += 1;
            sample(samples, &row.pk, format!("cbor: {error}"));
        }
    }
}

/// Audits one row of a known entity; returns the typed re-encoding.
fn sample(samples: &mut Vec<String>, pk: &str, reason: impl std::fmt::Display) {
    if samples.len() < 20 {
        samples.push(format!("{pk}: {reason}"));
    }
}

fn audit_row(
    stats: &mut EntityAudit,
    samples: &mut Vec<String>,
    descriptor: &EntityDescriptor,
    row: &Row,
    now_ms: i64,
) -> Option<Vec<u8>> {
    stats.rows += 1;
    if integer(&row.expires_at).is_some_and(|at| at <= now_ms) {
        stats.expired += 1;
    }
    let Some(bytes) = &row.data else {
        stats.null_data += 1;
        return None;
    };
    let original = match cbor::decode(bytes) {
        Ok(value) => value,
        Err(error) => {
            stats.cbor_failures += 1;
            sample(samples, &row.pk, format!("cbor: {error}"));
            return None;
        }
    };
    let typed = match (descriptor.roundtrip)(&original) {
        Ok(value) => value,
        Err(error) => {
            stats.typed_failures += 1;
            sample(samples, &row.pk, format!("typed: {error}"));
            return None;
        }
    };
    match (descriptor.recompute_pk)(&original) {
        Ok(pk) if pk == row.pk => {}
        Ok(pk) => {
            stats.pk_mismatches += 1;
            sample(samples, &row.pk, format!("recomputed key {pk}"));
        }
        Err(error) => {
            stats.pk_mismatches += 1;
            sample(samples, &row.pk, format!("key: {error}"));
        }
    }
    let encoded = cbor::encode(&typed);
    if &encoded == bytes {
        stats.byte_identical += 1;
    }
    let reread = cbor::decode(&encoded).unwrap_or(JsValue::Undefined);
    if !js_equal(&strip_undefined(&reread), &strip_undefined(&original)) {
        stats.value_diffs += 1;
        sample(samples, &row.pk, "typed round trip changed the value");
    }
    Some(encoded)
}

/// Parsed `compat-audit` arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditArgs {
    pub db: PathBuf,
    pub rewrite_to: Option<PathBuf>,
}

/// Runs the subcommand; `Ok(false)` when blocking issues were found.
pub async fn run(args: &AuditArgs, now_ms: i64) -> anyhow::Result<(bool, String)> {
    use anyhow::Context as _;
    let dir = tempfile_dir()?;
    let copy = dir.join("audit-copy.db");
    copy_database(&args.db, &copy)?;
    let conn = rusqlite::Connection::open(&copy).context("opening the copy")?;
    let rewrite = match &args.rewrite_to {
        Some(path) => {
            if path.exists() {
                anyhow::bail!(
                    "{} already exists; --rewrite-to needs a new file",
                    path.display()
                );
            }
            // Store::open creates the blobs schema exactly like mitools.
            let store = omni_store::Store::open(
                path,
                omni_store::StoreOptions::new(std::sync::Arc::new(omni_core::clock::SystemClock)),
            )
            .await
            .context("creating the rewrite database")?;
            drop(store);
            Some(rusqlite::Connection::open(path).context("opening the rewrite database")?)
        }
        None => None,
    };
    if let Some(target) = &rewrite {
        target.execute_batch("BEGIN")?;
    }
    let audit = audit(&conn, &entity_catalog(), now_ms, rewrite.as_ref())?;
    if let Some(target) = &rewrite {
        target.execute_batch("COMMIT")?;
    }
    let _ = std::fs::remove_dir_all(&dir);
    let mut report = audit.report();
    if let Some(path) = &args.rewrite_to {
        report.push_str(&format!(
            "next: cargo xtask node-readback {} {}\n",
            args.db.display(),
            path.display()
        ));
    }
    Ok((audit.blocking() == 0, report))
}

fn tempfile_dir() -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("omni-compat-audit-{}", omni_core::ids::uuid_v4()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Copies the database and its WAL/SHM side files (a WAL-mode copy may hold
/// committed pages only in `-wal`).
fn copy_database(from: &Path, to: &Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    std::fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", from.display()));
        if side.exists() {
            std::fs::copy(&side, PathBuf::from(format!("{}{suffix}", to.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn undefined_fields_and_number_widths_do_not_count_as_diffs() {
        let mut a = IndexMap::new();
        a.insert("n".to_owned(), JsValue::Int(3));
        a.insert("gone".to_owned(), JsValue::Undefined);
        let mut b = IndexMap::new();
        b.insert("n".to_owned(), JsValue::Float(3.0));
        assert!(js_equal(
            &strip_undefined(&JsValue::Object(a)),
            &strip_undefined(&JsValue::Object(b))
        ));
    }

    #[test]
    fn catalog_has_unique_names() {
        let catalog = entity_catalog();
        let mut names: Vec<_> = catalog.iter().map(|d| d.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
        assert!(names.contains(&"task-run"));
        assert!(names.contains(&"streamer-status"));
    }
}
