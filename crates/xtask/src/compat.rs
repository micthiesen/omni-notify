//! `compat-audit`: raw docstore compatibility of a production copy (section 4.5 step 3,
//! row level). Typed per-entity decoding and `recompute_pk` checks need every
//! subsystem's `EntityDescriptor` and live in the `omni-notify compat-audit` subcommand
//! (WP14); this command covers what the foundation can check on its own.
//!
//! The database is copied to a temporary directory first; the original is never opened.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use omni_store::cbor;

use crate::flag_value;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EntityStats {
    pub rows: u64,
    pub expired: u64,
    pub null_data: u64,
    pub decode_failures: u64,
    /// `encode(decode(bytes)) == bytes`.
    pub byte_identical: u64,
}

#[derive(Debug, Default)]
pub struct Audit {
    /// Keyed by entity name; `<null>` for rows without one.
    pub entities: BTreeMap<String, EntityStats>,
    /// `$`-prefixed rows whose entity column is NULL (pre-`migrateAll` legacy rows).
    pub legacy_null_entity: u64,
    pub sample_failures: Vec<String>,
}

fn integer(value: rusqlite::types::Value) -> Option<i64> {
    match value {
        rusqlite::types::Value::Integer(i) => Some(i),
        #[allow(clippy::cast_possible_truncation)]
        rusqlite::types::Value::Real(f) => Some(f as i64),
        _ => None,
    }
}

pub fn audit(conn: &rusqlite::Connection, now_ms: i64) -> Result<Audit> {
    let mut audit = Audit::default();
    let mut statement = conn.prepare("SELECT pk, data, entity, expires_at FROM blobs")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let pk: String = row.get("pk")?;
        let data: Option<Vec<u8>> = row.get("data")?;
        let entity: Option<String> = row.get("entity")?;
        let expires_at = integer(row.get("expires_at")?);
        if entity.is_none() && pk.starts_with('$') {
            audit.legacy_null_entity += 1;
        }
        let stats = audit
            .entities
            .entry(entity.unwrap_or_else(|| "<null>".to_owned()))
            .or_default();
        stats.rows += 1;
        if expires_at.is_some_and(|at| at <= now_ms) {
            stats.expired += 1;
        }
        let Some(data) = data else {
            stats.null_data += 1;
            continue;
        };
        match cbor::decode(&data) {
            Ok(value) => {
                if cbor::encode(&value) == data {
                    stats.byte_identical += 1;
                }
            }
            Err(error) => {
                stats.decode_failures += 1;
                if audit.sample_failures.len() < 20 {
                    audit.sample_failures.push(format!("{pk}: {error}"));
                }
            }
        }
    }
    Ok(audit)
}

pub fn compat_audit(args: &[String]) -> Result<()> {
    let source = flag_value(args, "--db").context("--db COPY is required")?;
    let strict = args.iter().any(|a| a == "--strict");
    let scratch = tempfile_dir()?;
    let copy = scratch.join("audit.db");
    std::fs::copy(source, &copy).with_context(|| format!("copying {source}"))?;
    for suffix in ["-wal", "-shm"] {
        let side = format!("{source}{suffix}");
        if std::path::Path::new(&side).exists() {
            std::fs::copy(&side, scratch.join(format!("audit.db{suffix}")))?;
        }
    }
    let conn = rusqlite::Connection::open(&copy)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let audit = audit(&conn, i64::try_from(now)?)?;
    drop(conn);
    std::fs::remove_dir_all(&scratch).ok();

    println!(
        "{:<40} {:>8} {:>8} {:>6} {:>8} {:>10}",
        "entity", "rows", "expired", "null", "corrupt", "identical"
    );
    let mut failures = 0;
    for (entity, s) in &audit.entities {
        failures += s.decode_failures;
        println!(
            "{entity:<40} {:>8} {:>8} {:>6} {:>8} {:>10}",
            s.rows, s.expired, s.null_data, s.decode_failures, s.byte_identical
        );
    }
    println!("legacy NULL-entity `$` rows: {}", audit.legacy_null_entity);
    for failure in &audit.sample_failures {
        println!("decode failure: {failure}");
    }
    if strict && failures > 0 {
        bail!("{failures} rows failed to decode");
    }
    Ok(())
}

fn tempfile_dir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("omni-compat-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audits_rows_by_entity() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs (pk TEXT PRIMARY KEY, data BLOB, entity TEXT, version INTEGER, expires_at INTEGER, updated_at INTEGER);",
        )
        .unwrap();
        let doc = cbor::encode(&cbor::JsValue::Object(
            [("a".to_owned(), cbor::JsValue::Int(1))]
                .into_iter()
                .collect(),
        ));
        let insert = "INSERT INTO blobs (pk, data, entity, version, expires_at, updated_at) VALUES (?1, ?2, ?3, 0, ?4, 0)";
        conn.execute(insert, rusqlite::params!["$x#s1:a", doc, "x", None::<i64>])
            .unwrap();
        conn.execute(
            insert,
            rusqlite::params!["$x#s1:b", vec![0xffu8], "x", 5i64],
        )
        .unwrap();
        conn.execute(
            insert,
            rusqlite::params!["$y#s1:c", doc, None::<String>, None::<i64>],
        )
        .unwrap();
        let audit = audit(&conn, 10).unwrap();
        let x = &audit.entities["x"];
        assert_eq!(
            (x.rows, x.expired, x.decode_failures, x.byte_identical),
            (2, 1, 1, 1)
        );
        assert_eq!(audit.legacy_null_entity, 1);
    }
}
