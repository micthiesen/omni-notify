//! Docstore behavior: reads, writes, expiry, prefix matching and schema setup.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use indexmap::IndexMap;
use omni_core::clock::Clock;
use omni_store::cbor::{self, JsValue};
use omni_store::{DocMeta, DocOps, DocWrite, RawRow, Store, StoreError, StoreOptions, like_prefix};
use rusqlite::{Connection, params};
use tempfile::TempDir;

const T0: i64 = 1_760_000_000_000;

#[derive(Debug)]
struct FixedClock(AtomicI64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Fixture {
    store: Store,
    clock: Arc<FixedClock>,
    dir: TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        Self::at(dir).await
    }

    async fn at(dir: TempDir) -> Self {
        let clock = Arc::new(FixedClock(AtomicI64::new(T0)));
        let store = Store::open(
            &dir.path().join("docstore.db"),
            StoreOptions::new(clock.clone()),
        )
        .await
        .expect("open");
        Self { store, clock, dir }
    }

    fn advance(&self, ms: i64) {
        self.clock.0.fetch_add(ms, Ordering::SeqCst);
    }

    async fn put(&self, pk: &str, value: JsValue, meta: DocMeta) {
        let pk = pk.to_owned();
        self.store
            .write(move |tx| tx.upsert_doc(&pk, &value, meta))
            .await
            .expect("upsert");
    }

    async fn raw_insert(&self, pk: &str, data: Option<Vec<u8>>) {
        let pk = pk.to_owned();
        self.store
            .write(move |tx| {
                tx.connection()
                    .execute(
                        "INSERT INTO blobs (pk, entity, data) VALUES (?1, 'thing', ?2)",
                        params![pk, data],
                    )
                    .map(|_| ())
                    .map_err(|e| StoreError::Sqlite(e.to_string()))
            })
            .await
            .expect("raw insert");
    }
}

fn obj(entries: &[(&str, JsValue)]) -> JsValue {
    JsValue::Object(
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect::<IndexMap<_, _>>(),
    )
}

fn s(v: &str) -> JsValue {
    JsValue::String(v.to_owned())
}

#[tokio::test]
async fn stores_retrieves_and_deletes_documents() {
    let f = Fixture::new().await;
    let doc = obj(&[
        ("a", s("x")),
        ("n", JsValue::Int(1)),
        ("u", JsValue::Undefined),
    ]);
    f.put("k1", doc.clone(), DocMeta::default()).await;
    let read = f.store.read(|d| d.get_doc("k1")).await.expect("read");
    // `undefined` properties are not written; the value reads back the same.
    assert_eq!(read, Some(obj(&[("a", s("x")), ("n", JsValue::Int(1))])));
    assert!(read.is_some_and(|read| cbor::same_value(&read, &doc)));
    assert_eq!(
        f.store.read(|d| d.get_doc("missing")).await.expect("read"),
        None
    );
    assert!(f.store.read(|d| d.has_doc("k1")).await.expect("has"));
    assert!(
        f.store
            .write(|tx| tx.delete_doc("k1"))
            .await
            .expect("delete")
    );
    assert!(
        !f.store
            .write(|tx| tx.delete_doc("k1"))
            .await
            .expect("delete")
    );
    assert!(!f.store.read(|d| d.has_doc("k1")).await.expect("has"));
}

#[tokio::test]
async fn defaults_and_stores_metadata_columns() {
    let f = Fixture::new().await;
    f.put("bare", JsValue::Null, DocMeta::default()).await;
    f.put(
        "meta",
        JsValue::Null,
        DocMeta {
            entity: Some("thing".to_owned()),
            version: 3,
            expires_at: Some(T0 + 10),
            updated_at: Some(42),
        },
    )
    .await;
    let rows = f
        .store
        .read(|d| Ok((d.get_raw_row("bare")?, d.get_raw_row("meta")?)))
        .await
        .expect("read");
    let bare = rows.0.expect("bare");
    assert_eq!(
        (bare.entity, bare.version, bare.expires_at, bare.updated_at),
        (None, 0, None, T0)
    );
    assert_eq!(bare.data.as_deref(), Some(&[0xf6][..]));
    let meta = rows.1.expect("meta");
    assert_eq!(
        (
            meta.entity.as_deref(),
            meta.version,
            meta.expires_at,
            meta.updated_at
        ),
        (Some("thing"), 3, Some(T0 + 10), 42)
    );
}

#[tokio::test]
async fn prefix_and_entity_scoped_reads_counts_and_deletes() {
    let f = Fixture::new().await;
    for (pk, entity) in [("p:1", "a"), ("p:2", "a"), ("q:1", "b")] {
        f.put(
            pk,
            s(pk),
            DocMeta {
                entity: Some(entity.to_owned()),
                ..DocMeta::default()
            },
        )
        .await;
    }
    let (keys, docs, count, by_entity, entity_count) = f
        .store
        .read(|d| {
            Ok((
                d.get_keys_by_prefix("p:")?,
                d.get_docs_by_prefix("p:")?,
                d.count_by_prefix("p:")?,
                d.get_docs_by_entity("b")?,
                d.count_by_entity("a")?,
            ))
        })
        .await
        .expect("read");
    assert_eq!(keys, vec!["p:1", "p:2"]);
    assert_eq!(docs.len(), 2);
    assert_eq!(count, 2);
    assert_eq!(by_entity, vec![("q:1".to_owned(), s("q:1"))]);
    assert_eq!(entity_count, 2);
    assert_eq!(
        f.store
            .write(|tx| tx.delete_docs_by_prefix("p:"))
            .await
            .expect("delete"),
        2
    );
    assert_eq!(
        f.store
            .write(|tx| tx.delete_docs_by_entity("b"))
            .await
            .expect("delete"),
        1
    );
    assert_eq!(
        f.store
            .read(|d| d.count_by_prefix(""))
            .await
            .expect("count"),
        0
    );
}

#[tokio::test]
async fn expired_rows_are_absent_except_from_the_raw_prefix_scan() {
    let f = Fixture::new().await;
    let expiring = DocMeta {
        expires_at: Some(T0 + 1_000),
        ..DocMeta::default()
    };
    f.put("e:1", s("soon"), expiring.clone()).await;
    f.put("e:2", s("forever"), DocMeta::default()).await;
    assert!(f.store.read(|d| d.has_doc("e:1")).await.expect("has"));

    // Expiry is exclusive: a row whose expires_at equals now is gone.
    f.advance(1_000);
    let (doc, has, keys, docs, count, raw, raw_all) = f
        .store
        .read(|d| {
            Ok((
                d.get_doc("e:1")?,
                d.has_doc("e:1")?,
                d.get_keys_by_prefix("e:")?,
                d.get_docs_by_prefix("e:")?,
                d.count_by_prefix("e:")?,
                d.get_raw_row("e:1")?,
                d.get_raw_rows_by_prefix("e:")?,
            ))
        })
        .await
        .expect("read");
    assert_eq!(doc, None);
    assert!(!has);
    assert_eq!(keys, vec!["e:2"]);
    assert_eq!(docs.len(), 1);
    assert_eq!(count, 1);
    assert_eq!(raw, None);
    assert_eq!(raw_all.len(), 2);
    // touch refuses a dead row.
    assert!(
        !f.store
            .write(|tx| tx.touch_doc("e:1", None))
            .await
            .expect("touch")
    );
}

#[tokio::test]
async fn touch_and_cleanup_expired() {
    let f = Fixture::new().await;
    for i in 0..5 {
        f.put(
            &format!("x:{i}"),
            JsValue::Int(i),
            DocMeta {
                expires_at: Some(T0 + 10),
                ..DocMeta::default()
            },
        )
        .await;
    }
    f.put("x:live", JsValue::Null, DocMeta::default()).await;
    f.advance(5);
    assert!(
        f.store
            .write(|tx| tx.touch_doc("x:0", Some(T0 + 1_000)))
            .await
            .expect("touch")
    );
    let touched = f
        .store
        .read(|d| d.get_raw_row("x:0"))
        .await
        .expect("raw")
        .expect("row");
    assert_eq!(
        (touched.expires_at, touched.updated_at),
        (Some(T0 + 1_000), T0 + 5)
    );
    assert!(
        f.store
            .write(|tx| tx.touch_doc("x:0", None))
            .await
            .expect("clear expiry")
    );
    f.advance(10);
    assert_eq!(
        f.store
            .write(|tx| tx.cleanup_expired(2))
            .await
            .expect("cleanup"),
        2
    );
    assert_eq!(
        f.store
            .write(|tx| tx.cleanup_expired(1000))
            .await
            .expect("cleanup"),
        2
    );
    let raw = f
        .store
        .read(|d| d.get_raw_rows_by_prefix("x:"))
        .await
        .expect("raw");
    let mut pks: Vec<String> = raw.into_iter().map(|r| r.pk).collect();
    pks.sort();
    assert_eq!(pks, vec!["x:0", "x:live"]);
}

#[tokio::test]
async fn like_metacharacters_are_literal_and_ascii_matching_is_case_insensitive() {
    assert_eq!(like_prefix(r"a%b_c\d"), r"a\%b\_c\\d%");
    let f = Fixture::new().await;
    for pk in [
        "50%:x",
        "50:x",
        "a_b",
        "axb",
        r"back\slash",
        "backXslash",
        "$Pair#s1:x",
        "$pair#s1:y",
        "Éa",
        "éb",
    ] {
        f.put(pk, JsValue::Null, DocMeta::default()).await;
    }
    let keys = |prefix: &'static str| {
        let store = f.store.clone();
        async move {
            let mut keys = store
                .read(move |d| d.get_keys_by_prefix(prefix))
                .await
                .expect("keys");
            keys.sort();
            keys
        }
    };
    assert_eq!(keys("50%").await, vec!["50%:x"]);
    assert_eq!(keys("a_").await, vec!["a_b"]);
    assert_eq!(keys(r"back\").await, vec![r"back\slash"]);
    // SQLite LIKE folds ASCII case only.
    assert_eq!(keys("$pair#").await, vec!["$Pair#s1:x", "$pair#s1:y"]);
    assert_eq!(keys("é").await, vec!["éb"]);
}

#[tokio::test]
async fn transactions_commit_together_and_roll_back_on_error_or_panic() {
    let f = Fixture::new().await;
    f.store
        .write(|tx| {
            tx.upsert_doc("t:1", &JsValue::Int(1), DocMeta::default())?;
            tx.upsert_doc("t:2", &JsValue::Int(2), DocMeta::default())
        })
        .await
        .expect("commit");
    let failed: Result<(), StoreError> = f
        .store
        .write(|tx| {
            tx.upsert_doc("t:3", &JsValue::Int(3), DocMeta::default())?;
            tx.delete_doc("t:1")?;
            Err(StoreError::Validation {
                entity: "t",
                reason: "boom".to_owned(),
            })
        })
        .await;
    assert!(matches!(failed, Err(StoreError::Validation { .. })));
    let store = f.store.clone();
    let panicked = tokio::spawn(async move {
        store
            .write(|tx| -> Result<(), StoreError> {
                tx.upsert_doc("t:4", &JsValue::Int(4), DocMeta::default())?;
                panic!("job panics mid-transaction");
            })
            .await
    })
    .await;
    assert!(panicked.is_err_and(|e| e.is_panic()));
    let keys = f
        .store
        .read(|d| d.get_keys_by_prefix("t:"))
        .await
        .expect("store still usable");
    assert_eq!(keys, vec!["t:1", "t:2"]);
}

#[tokio::test]
async fn clears_every_row_and_round_trips_large_payloads() {
    let f = Fixture::new().await;
    let big = s(&"x".repeat(300 * 1024));
    f.put("big", big.clone(), DocMeta::default()).await;
    assert_eq!(
        f.store.read(|d| d.get_doc("big")).await.expect("read"),
        Some(big)
    );
    let bytes = f
        .store
        .read(|d| d.storage_bytes_by_prefix("bi"))
        .await
        .expect("bytes");
    assert!(bytes > 300 * 1024);
    assert!(
        f.store
            .read(|d| d.database_size_bytes())
            .await
            .expect("size")
            > 0
    );
    f.store.write(|tx| tx.clear()).await.expect("clear");
    assert_eq!(
        f.store
            .read(|d| d.count_by_prefix(""))
            .await
            .expect("count"),
        0
    );
}

#[tokio::test]
async fn corrupt_rows_fail_point_reads_and_are_skipped_by_collection_reads() {
    let f = Fixture::new().await;
    f.put(
        "c:good",
        s("ok"),
        DocMeta {
            entity: Some("thing".to_owned()),
            ..DocMeta::default()
        },
    )
    .await;
    f.raw_insert("c:truncated", Some(vec![0x62, 0x61])).await;
    f.raw_insert("c:null", None).await;
    f.raw_insert("c:empty", Some(Vec::new())).await;
    f.raw_insert("c:trailing", Some(vec![0x01, 0x02])).await;
    for pk in ["c:truncated", "c:null", "c:empty", "c:trailing"] {
        let res = f.store.read(move |d| d.get_doc(pk)).await;
        assert!(
            matches!(res, Err(StoreError::CorruptRow { .. })),
            "{pk}: {res:?}"
        );
        // Existence checks and raw rows never decode.
        assert!(f.store.read(move |d| d.has_doc(pk)).await.expect("has"));
        let raw: Option<RawRow> = f.store.read(move |d| d.get_raw_row(pk)).await.expect("raw");
        assert!(raw.expect("row").decode().is_err());
    }
    let by_prefix = f
        .store
        .read(|d| d.get_docs_by_prefix("c:"))
        .await
        .expect("prefix");
    assert_eq!(by_prefix, vec![("c:good".to_owned(), s("ok"))]);
    let by_entity = f
        .store
        .read(|d| d.get_docs_by_entity("thing"))
        .await
        .expect("entity");
    assert_eq!(by_entity, vec![("c:good".to_owned(), s("ok"))]);
}

#[tokio::test]
async fn initializes_schema_and_indexes() {
    let f = Fixture::new().await;
    let path = f.dir.path().join("docstore.db");
    drop(f.store);
    let conn = Connection::open(path).expect("open");
    let mut names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE tbl_name = 'blobs' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names.retain(|n| !n.starts_with("sqlite_autoindex"));
    assert_eq!(names, vec!["blobs", "blobs_entity_idx", "blobs_expiry_idx"]);
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[tokio::test]
async fn upgrades_a_legacy_table_with_production_column_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let conn = Connection::open(dir.path().join("docstore.db")).expect("open");
        // The original two-column table, as production still has it (the
        // metadata columns were appended by ALTER TABLE).
        conn.execute_batch(
            "CREATE TABLE blobs (
      pk   TEXT PRIMARY KEY,
      data BLOB
    );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO blobs (pk, data) VALUES ('$legacy#x', ?1)",
            params![cbor::encode(&s("old"))],
        )
        .unwrap();
    }
    let f = Fixture::at(dir).await;
    let row = f
        .store
        .read(|d| d.get_raw_row("$legacy#x"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(
        (
            row.entity.clone(),
            row.version,
            row.expires_at,
            row.updated_at
        ),
        (None, 0, None, 0)
    );
    assert_eq!(row.decode().expect("decode"), s("old"));
    let path = f.dir.path().join("docstore.db");
    let columns: Vec<String> = Connection::open(path)
        .unwrap()
        .prepare("PRAGMA table_info(blobs)")
        .unwrap()
        .query_map([], |r| r.get("name"))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        columns,
        vec![
            "pk",
            "data",
            "entity",
            "version",
            "expires_at",
            "updated_at"
        ]
    );
}

#[tokio::test]
async fn integer_columns_accept_real_values() {
    let f = Fixture::new().await;
    f.store
        .write(|tx| {
            tx.connection()
                .execute(
                    "INSERT INTO blobs (pk, version, expires_at, updated_at, data)
                     VALUES ('r', 2.0, ?1, 1.5e12, x'f6')",
                    params![(T0 + 500) as f64 + 0.5],
                )
                .map(|_| ())
                .map_err(|e| StoreError::Sqlite(e.to_string()))
        })
        .await
        .expect("insert");
    let row = f
        .store
        .read(|d| d.get_raw_row("r"))
        .await
        .expect("read")
        .expect("row");
    assert_eq!(
        (row.version, row.expires_at, row.updated_at),
        (2, Some(T0 + 500), 1_500_000_000_000)
    );
    f.advance(501);
    assert!(!f.store.read(|d| d.has_doc("r")).await.expect("has"));
}

#[tokio::test]
async fn backup_produces_a_readable_copy() {
    let f = Fixture::new().await;
    f.put("b:1", s("v"), DocMeta::default()).await;
    let dest = f.dir.path().join("backup.db");
    f.store.backup_to(&dest).await.expect("backup");
    let copy = Store::open(&dest, StoreOptions::new(f.clock.clone()))
        .await
        .expect("open backup");
    assert_eq!(
        copy.read(|d| d.get_doc("b:1")).await.expect("read"),
        Some(s("v"))
    );
}
