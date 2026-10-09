//! Typed entities, serde bridge, migrate_all and relational tables (ports
//! mitools `entities.spec.ts` and `table.spec.ts`).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use indexmap::IndexMap;
use omni_core::clock::Clock;
use omni_store::cbor::{self, Extra, JsDate, JsValue};
use omni_store::entity::{
    self, EntityDescriptor, JsObjectPatch, KeyPart, ModifyOpts, UpsertOpts, migrate_all,
};
use omni_store::table::SqlValue;
use omni_store::table::pets::{PetRow, WeightHistoryRow};
use omni_store::{
    DocMeta, DocOps, DocWrite, Entity, EntityOps, EntityWrite, Store, StoreError, StoreOptions,
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

const T0: i64 = 1_760_000_000_000;

#[derive(Debug)]
struct FixedClock(AtomicI64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

async fn open() -> (Store, Arc<FixedClock>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = Arc::new(FixedClock(AtomicI64::new(T0)));
    let store = Store::open(
        &dir.path().join("docstore.db"),
        StoreOptions::new(clock.clone()),
    )
    .await
    .expect("open");
    (store, clock, dir)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    streamer_id: String,
    started: f64,
    title: String,
    viewers: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    cost_cents: Option<f64>,
    #[serde(flatten)]
    extra: Extra,
}

impl Entity for Session {
    const NAME: &'static str = "test-session";
    const VERSION: i64 = 2;
    type Key = (String, f64);
    fn key(&self) -> Self::Key {
        (self.streamer_id.clone(), self.started)
    }
    fn validate(&self) -> Result<(), String> {
        if self.viewers < 0 {
            return Err("viewers must be non-negative".to_owned());
        }
        Ok(())
    }
    fn migrate(mut raw: JsValue, from: i64) -> Result<JsValue, String> {
        let object = raw.as_object_mut().ok_or("not an object")?;
        if from < 2 && !object.contains_key("title") {
            object.insert("title".to_owned(), JsValue::String("untitled".to_owned()));
        }
        if from < 1 && !object.contains_key("viewers") {
            object.insert("viewers".to_owned(), JsValue::Int(0));
        }
        if !object.contains_key("costCents") {
            object.insert("costCents".to_owned(), JsValue::Null);
        }
        Ok(raw)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Delivery {
    id: String,
    #[serde(flatten)]
    extra: Extra,
}

impl Entity for Delivery {
    const NAME: &'static str = "test-delivery";
    const DEFAULT_TTL_MS: Option<i64> = Some(90 * 86_400_000);
    type Key = String;
    fn key(&self) -> String {
        self.id.clone()
    }
}

fn session(id: &str, started: f64, viewers: i64) -> Session {
    Session {
        streamer_id: id.to_owned(),
        started,
        title: format!("{id} live"),
        viewers,
        note: None,
        cost_cents: None,
        extra: Extra::new(),
    }
}

#[tokio::test]
async fn round_trips_and_stamps_entity_metadata() {
    let (store, _, _dir) = open().await;
    let s = session("dest😀", 1.5, 10);
    let stored = s.clone();
    store
        .write(move |tx| tx.upsert(&stored, UpsertOpts::default()))
        .await
        .expect("upsert");
    let pk = entity::pk::<Session>(&("dest😀".to_owned(), 1.5)).expect("pk");
    assert_eq!(pk, "$test-session#s6:dest😀#n1.5");
    let (got, row) = store
        .read(move |d| {
            Ok((
                d.get::<Session>(&("dest😀".to_owned(), 1.5))?,
                d.get_raw_row(&pk)?,
            ))
        })
        .await
        .expect("read");
    assert_eq!(got, Some(s));
    let row = row.expect("row");
    assert_eq!(
        (
            row.entity.as_deref(),
            row.version,
            row.expires_at,
            row.updated_at
        ),
        (Some("test-session"), 2, None, T0)
    );
    // `note` (TS `note?`) is omitted; `costCents` (TS `T | null`) is written as null.
    let JsValue::Object(map) = row.decode().expect("decode") else {
        panic!("object")
    };
    assert_eq!(
        map.keys().collect::<Vec<_>>(),
        vec!["streamerId", "started", "title", "viewers", "costCents"]
    );
    assert_eq!(map["costCents"], JsValue::Null);
}

#[tokio::test]
async fn deletes_counts_lists_and_scopes_by_entity() {
    let (store, _, _dir) = open().await;
    store
        .write(|tx| {
            for (id, started) in [("a", 1.0), ("a", 2.0), ("b", 1.0)] {
                tx.upsert(&session(id, started, 1), UpsertOpts::default())?;
            }
            tx.upsert(
                &Delivery {
                    id: "neighbour".to_owned(),
                    extra: Extra::new(),
                },
                UpsertOpts::default(),
            )
        })
        .await
        .expect("seed");
    let (all, a_only, count, has) = store
        .read(|d| {
            Ok((
                d.get_all::<Session>()?,
                d.get_by_prefix::<Session>(&[KeyPart::from("a")])?,
                d.count::<Session>()?,
                d.has::<Session>(&("b".to_owned(), 1.0))?,
            ))
        })
        .await
        .expect("read");
    assert_eq!(all.len(), 3);
    assert_eq!(a_only.len(), 2);
    assert_eq!(count, 3);
    assert!(has);
    assert!(
        store
            .write(|tx| tx.delete::<Session>(&("b".to_owned(), 1.0)))
            .await
            .expect("delete")
    );
    assert_eq!(
        store
            .write(|tx| tx.delete_all::<Session>())
            .await
            .expect("delete all"),
        2
    );
    assert_eq!(
        store.read(|d| d.count::<Delivery>()).await.expect("count"),
        1
    );
}

#[tokio::test]
async fn key_parts_never_collide() {
    #[derive(Serialize, Deserialize)]
    struct Pair {
        a: String,
        b: String,
    }
    impl Entity for Pair {
        const NAME: &'static str = "pair";
        type Key = (String, String);
        fn key(&self) -> Self::Key {
            (self.a.clone(), self.b.clone())
        }
    }
    let one = entity::pk::<Pair>(&("a#s1:b".to_owned(), "c".to_owned())).expect("pk");
    let two = entity::pk::<Pair>(&("a".to_owned(), "b#s1:c".to_owned())).expect("pk");
    assert_ne!(one, two);
    assert_eq!(one, "$pair#s6:a#s1:b#s1:c");
    assert_eq!(
        entity::pk::<Session>(&("x".to_owned(), 1e21)).expect("pk"),
        "$test-session#s1:x#n1e+21"
    );
    assert!(matches!(
        entity::pk::<Session>(&("x".to_owned(), f64::INFINITY)),
        Err(StoreError::InvalidKey(_))
    ));
}

#[tokio::test]
async fn validation_rejects_upserts_and_modifies() {
    let (store, _, _dir) = open().await;
    let bad = session("v", 1.0, -1);
    let res = store
        .write(move |tx| tx.upsert(&bad, UpsertOpts::default()))
        .await;
    assert!(matches!(
        res,
        Err(StoreError::Validation {
            entity: "test-session",
            ..
        })
    ));
    store
        .write(|tx| tx.upsert(&session("v", 1.0, 1), UpsertOpts::default()))
        .await
        .expect("upsert");
    let res = store
        .write(|tx| {
            tx.update::<Session>(
                &("v".to_owned(), 1.0),
                |mut s| {
                    s.viewers = -5;
                    s
                },
                ModifyOpts::default(),
            )
        })
        .await;
    assert!(matches!(res, Err(StoreError::Validation { .. })));
    let mut patch = JsObjectPatch::new();
    patch.insert("viewers".to_owned(), JsValue::Int(-2));
    let res = store
        .write(move |tx| tx.patch::<Session>(&("v".to_owned(), 1.0), patch, ModifyOpts::default()))
        .await;
    assert!(matches!(res, Err(StoreError::Validation { .. })));
    let current = store
        .read(|d| d.get::<Session>(&("v".to_owned(), 1.0)))
        .await
        .expect("read");
    assert_eq!(current.map(|s| s.viewers), Some(1));
}

#[tokio::test]
async fn patch_and_update_modify_in_place_and_cannot_move_the_row() {
    let (store, _, _dir) = open().await;
    store
        .write(|tx| tx.upsert(&session("p", 1.0, 1), UpsertOpts::default()))
        .await
        .expect("seed");
    let mut patch = JsObjectPatch::new();
    patch.insert("title".to_owned(), JsValue::String("patched".to_owned()));
    patch.insert("unknownField".to_owned(), JsValue::Undefined);
    let patched = store
        .write(move |tx| tx.patch::<Session>(&("p".to_owned(), 1.0), patch, ModifyOpts::default()))
        .await
        .expect("patch")
        .expect("present");
    assert_eq!(patched.title, "patched");
    assert_eq!(patched.extra.get("unknownField"), Some(&JsValue::Undefined));

    let updated = store
        .write(|tx| {
            tx.update::<Session>(
                &("p".to_owned(), 1.0),
                |mut s| {
                    s.viewers += 41;
                    s
                },
                ModifyOpts::default(),
            )
        })
        .await
        .expect("update")
        .expect("present");
    assert_eq!(updated.viewers, 42);
    assert_eq!(updated.extra.get("unknownField"), Some(&JsValue::Undefined));

    let mut moving = JsObjectPatch::new();
    moving.insert(
        "streamerId".to_owned(),
        JsValue::String("elsewhere".to_owned()),
    );
    let res = store
        .write(move |tx| tx.patch::<Session>(&("p".to_owned(), 1.0), moving, ModifyOpts::default()))
        .await;
    assert!(matches!(res, Err(StoreError::Validation { .. })));

    let missing = store
        .write(|tx| {
            tx.patch::<Session>(
                &("nope".to_owned(), 1.0),
                JsObjectPatch::new(),
                ModifyOpts::default(),
            )
        })
        .await
        .expect("patch missing");
    assert_eq!(missing, None);
}

#[tokio::test]
async fn ttl_rules_follow_mitools() {
    let (store, clock, _dir) = open().await;
    let delivery = |id: &str| Delivery {
        id: id.to_owned(),
        extra: Extra::new(),
    };
    let (d1, d2, d3) = (delivery("default"), delivery("ttl"), delivery("explicit"));
    store
        .write(move |tx| {
            tx.upsert(&d1, UpsertOpts::default())?;
            tx.upsert(
                &d2,
                UpsertOpts {
                    ttl_ms: Some(1_000),
                    expires_at: None,
                },
            )?;
            tx.upsert(
                &d3,
                UpsertOpts {
                    ttl_ms: Some(1_000),
                    expires_at: Some(T0 + 5),
                },
            )
        })
        .await
        .expect("upsert");
    let expiry = |pk: &'static str| {
        let store = store.clone();
        async move {
            store
                .read(move |d| d.get_raw_row(pk))
                .await
                .expect("read")
                .map(|r| r.expires_at)
        }
    };
    assert_eq!(
        expiry("$test-delivery#s7:default").await,
        Some(Some(T0 + 90 * 86_400_000))
    );
    assert_eq!(
        expiry("$test-delivery#s3:ttl").await,
        Some(Some(T0 + 1_000))
    );
    assert_eq!(
        expiry("$test-delivery#s8:explicit").await,
        Some(Some(T0 + 5))
    );

    // Modifies keep the expiry (never re-applying the default) unless asked.
    clock.0.fetch_add(1, Ordering::SeqCst);
    store
        .write(|tx| tx.update::<Delivery>(&"ttl".to_owned(), |d| d, ModifyOpts::default()))
        .await
        .expect("update");
    assert_eq!(
        expiry("$test-delivery#s3:ttl").await,
        Some(Some(T0 + 1_000))
    );
    store
        .write(|tx| {
            tx.patch::<Delivery>(
                &"ttl".to_owned(),
                JsObjectPatch::new(),
                ModifyOpts {
                    expires_at: Some(None),
                    ttl_ms: None,
                },
            )
        })
        .await
        .expect("clear");
    assert_eq!(expiry("$test-delivery#s3:ttl").await, Some(None));
    store
        .write(|tx| {
            tx.update::<Delivery>(
                &"ttl".to_owned(),
                |d| d,
                ModifyOpts {
                    expires_at: None,
                    ttl_ms: Some(50),
                },
            )
        })
        .await
        .expect("ttl");
    assert_eq!(expiry("$test-delivery#s3:ttl").await, Some(Some(T0 + 51)));

    // Touch: live rows only, default TTL when no option is given.
    assert!(
        store
            .write(|tx| tx.touch::<Delivery>(&"ttl".to_owned(), UpsertOpts::default()))
            .await
            .expect("touch")
    );
    assert_eq!(
        expiry("$test-delivery#s3:ttl").await,
        Some(Some(T0 + 1 + 90 * 86_400_000))
    );
    clock.0.fetch_add(10, Ordering::SeqCst);
    assert_eq!(
        store
            .read(|d| d.get::<Delivery>(&"explicit".to_owned()))
            .await
            .expect("read"),
        None
    );
    assert!(
        !store
            .write(|tx| tx.touch::<Delivery>(&"explicit".to_owned(), UpsertOpts::default()))
            .await
            .expect("touch")
    );
}

#[tokio::test]
async fn corrupt_and_mistyped_rows() {
    let (store, _, _dir) = open().await;
    store
        .write(|tx| tx.upsert(&session("ok", 1.0, 1), UpsertOpts::default()))
        .await
        .expect("seed");
    let bad_pk = entity::pk::<Session>(&("bad".to_owned(), 1.0)).expect("pk");
    let mistyped_pk = entity::pk::<Session>(&("typed".to_owned(), 1.0)).expect("pk");
    let (bad, mistyped) = (bad_pk.clone(), mistyped_pk.clone());
    store
        .write(move |tx| {
            let meta = DocMeta { entity: Some("test-session".to_owned()), version: 2, ..DocMeta::default() };
            tx.connection()
                .execute(
                    "INSERT INTO blobs (pk, entity, version, data) VALUES (?1, 'test-session', 2, x'62')",
                    [&bad],
                )
                .map_err(|e| StoreError::Sqlite(e.to_string()))?;
            tx.upsert_doc(&mistyped, &JsValue::String("not an object".to_owned()), meta)
        })
        .await
        .expect("seed corrupt");
    let point = store
        .read(|d| d.get::<Session>(&("bad".to_owned(), 1.0)))
        .await;
    assert!(matches!(point, Err(StoreError::CorruptRow { .. })));
    let typed = store
        .read(|d| d.get::<Session>(&("typed".to_owned(), 1.0)))
        .await;
    assert!(matches!(typed, Err(StoreError::Decode { .. })));
    let all = store
        .read(|d| d.get_all::<Session>())
        .await
        .expect("get_all skips");
    assert_eq!(all.len(), 1);
    let update = store
        .write(|tx| tx.update::<Session>(&("bad".to_owned(), 1.0), |s| s, ModifyOpts::default()))
        .await;
    assert!(matches!(update, Err(StoreError::CorruptRow { .. })));
}

#[tokio::test]
async fn migrate_all_rewrites_legacy_rows_and_isolates_failures() {
    let (store, _, _dir) = open().await;
    let legacy = |id: &str, started: f64| {
        let mut map = IndexMap::new();
        map.insert("streamerId".to_owned(), JsValue::String(id.to_owned()));
        map.insert("started".to_owned(), JsValue::Float(started));
        JsValue::Object(map)
    };
    let current_pk = entity::pk::<Session>(&("live".to_owned(), 1.0)).expect("pk");
    let (l1, l2) = (legacy("old", 3.0), legacy("live", 1.0));
    let live = session("live", 1.0, 99);
    store
        .write(move |tx| {
            // Legacy key encoding, NULL entity, version 0, updated_at 0, with an expiry.
            tx.connection()
                .execute(
                    "INSERT INTO blobs (pk, data, expires_at) VALUES ('$test-session#old#3', ?1, 5)",
                    [cbor::encode(&l1)],
                )
                .map_err(|e| StoreError::Sqlite(e.to_string()))?;
            // A stale legacy duplicate of a row that already exists under the new key.
            tx.connection()
                .execute(
                    "INSERT INTO blobs (pk, data, updated_at) VALUES ('$test-session#live#1', ?1, 7)",
                    [cbor::encode(&l2)],
                )
                .map_err(|e| StoreError::Sqlite(e.to_string()))?;
            // Undecodable.
            tx.connection()
                .execute("INSERT INTO blobs (pk, data) VALUES ('$test-session#junk', x'ff')", [])
                .map_err(|e| StoreError::Sqlite(e.to_string()))?;
            tx.upsert(&live, UpsertOpts::default())
        })
        .await
        .expect("seed");

    let report = store
        .write(|tx| {
            migrate_all(
                tx,
                &[
                    EntityDescriptor::of::<Session>(),
                    EntityDescriptor::of::<Delivery>(),
                ],
            )
        })
        .await
        .expect("migrate");
    assert_eq!(report.migrated, 1);
    assert_eq!(report.collisions_skipped, 1);
    assert_eq!(report.failed, 1);

    let migrated_pk = entity::pk::<Session>(&("old".to_owned(), 3.0)).expect("pk");
    let (migrated, old_gone, stale_kept, live_row) = store
        .read(move |d| {
            Ok((
                d.get_raw_rows_by_prefix(&migrated_pk)?,
                d.get_raw_rows_by_prefix("$test-session#old#3")?,
                d.get_raw_rows_by_prefix("$test-session#live#1")?,
                d.get::<Session>(&("live".to_owned(), 1.0))?,
            ))
        })
        .await
        .expect("read");
    let row = &migrated[0];
    assert_eq!(
        (
            row.entity.as_deref(),
            row.version,
            row.expires_at,
            row.updated_at
        ),
        (Some("test-session"), 2, Some(5), T0)
    );
    let value: Session = cbor::from_value(row.decode().expect("decode")).expect("typed");
    assert_eq!((value.title.as_str(), value.viewers), ("untitled", 0));
    assert!(old_gone.is_empty());
    assert_eq!(stale_kept.len(), 1);
    assert_eq!(live_row.map(|s| s.viewers), Some(99));
    assert!(current_pk.starts_with("$test-session#s4:live"));

    // Idempotent.
    let again = store
        .write(|tx| migrate_all(tx, &[EntityDescriptor::of::<Session>()]))
        .await
        .expect("migrate again");
    assert_eq!(
        (again.migrated, again.collisions_skipped, again.failed),
        (0, 1, 1)
    );
}

#[tokio::test]
async fn pets_tables() {
    let (store, _, _dir) = open().await;
    let pets = store.table::<PetRow>().await.expect("pets");
    let history = store.table::<WeightHistoryRow>().await.expect("history");
    store.table::<PetRow>().await.expect("idempotent");
    let pet = PetRow {
        pet_id: "p1".to_owned(),
        name: "Mochi".to_owned(),
        current_weight: 4.2,
        updated_at: "2026-10-01T00:00:00.000Z".to_owned(),
    };
    pets.upsert(&pet).await.expect("upsert");
    pets.upsert(&PetRow {
        current_weight: 4.4,
        ..pet.clone()
    })
    .await
    .expect("replace");
    assert_eq!(
        pets.all()
            .await
            .expect("all")
            .iter()
            .map(|p| p.current_weight)
            .collect::<Vec<_>>(),
        vec![4.4]
    );
    let reading = |ts: &str, w: f64| WeightHistoryRow {
        pet_id: "p1".to_owned(),
        timestamp: ts.to_owned(),
        weight: w,
    };
    assert!(
        history
            .insert(&reading("2026-10-02", 4.3))
            .await
            .expect("insert")
    );
    assert!(
        !history
            .insert(&reading("2026-10-02", 9.9))
            .await
            .expect("ignored duplicate")
    );
    assert!(
        history
            .insert(&reading("2026-10-01", 4.1))
            .await
            .expect("insert")
    );
    let rows = history
        .query(
            "pet_id = ? ORDER BY timestamp ASC",
            vec![SqlValue::Text("p1".to_owned())],
        )
        .await
        .expect("query");
    assert_eq!(
        rows.iter().map(|r| r.weight).collect::<Vec<_>>(),
        vec![4.1, 4.3]
    );
    assert!(history.query("nonsense ===", Vec::new()).await.is_err());
    history.clear().await.expect("clear");
    assert!(history.all().await.expect("all").is_empty());
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rich {
    at: JsDate,
    maybe: Option<String>,
    kind: Kind,
    tags: Vec<String>,
    count: u32,
    ratio: f64,
    #[serde(flatten)]
    extra: Extra,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Movie,
    Show,
}

#[test]
fn serde_bridge_preserves_js_values() {
    let mut extra = IndexMap::new();
    extra.insert("undef".to_owned(), JsValue::Undefined);
    extra.insert("when".to_owned(), JsValue::Date(1_500.0));
    extra.insert("set".to_owned(), JsValue::Set(vec![JsValue::Int(1)]));
    extra.insert("big".to_owned(), JsValue::BigInt(1 << 70));
    extra.insert("simple".to_owned(), JsValue::Simple(16));
    extra.insert(
        "tagged".to_owned(),
        JsValue::Tagged(1000, Box::new(JsValue::Bytes(vec![1]))),
    );
    extra.insert(
        "map".to_owned(),
        JsValue::Map(vec![(
            JsValue::Int(1),
            JsValue::Array(vec![JsValue::Undefined]),
        )]),
    );
    extra.insert(
        "nested".to_owned(),
        JsValue::Object(IndexMap::from([("u".to_owned(), JsValue::Undefined)])),
    );
    let mut source = IndexMap::new();
    source.insert("at".to_owned(), JsValue::Date(T0 as f64));
    source.insert("maybe".to_owned(), JsValue::Undefined);
    source.insert("kind".to_owned(), JsValue::String("show".to_owned()));
    source.insert(
        "tags".to_owned(),
        JsValue::Array(vec![JsValue::String("a".to_owned())]),
    );
    source.insert("count".to_owned(), JsValue::Float(3.0));
    source.insert("ratio".to_owned(), JsValue::Int(2));
    source.extend(extra.clone());
    let rich: Rich = cbor::from_value(JsValue::Object(source)).expect("typed");
    assert_eq!(rich.at, JsDate(T0));
    assert_eq!(rich.maybe, None);
    assert_eq!(rich.kind, Kind::Show);
    assert_eq!((rich.count, rich.ratio), (3, 2.0));
    assert_eq!(rich.extra, extra);

    let back = cbor::to_value(&rich).expect("to_value");
    let JsValue::Object(map) = &back else {
        panic!("object")
    };
    assert_eq!(map["at"], JsValue::Date(T0 as f64));
    assert_eq!(map["maybe"], JsValue::Null);
    for (key, value) in &extra {
        assert_eq!(&map[key], value, "{key}");
    }
    // Encoded bytes: the Date is tag 1 over whole seconds (a u32 here).
    assert_eq!(&cbor::encode(&map["at"])[..2], &[0xc1, 0x1a]);

    // Typed numbers reject JS values they cannot hold.
    assert!(cbor::from_value::<u32>(JsValue::Float(1.5)).is_err());
    assert!(cbor::from_value::<i64>(JsValue::Int(1 << 60)).is_err());
    assert!(cbor::from_value::<String>(JsValue::Undefined).is_err());
    assert_eq!(
        cbor::from_value::<Option<u8>>(JsValue::Undefined).expect("none"),
        None
    );
    assert_eq!(
        cbor::from_value::<JsDate>(JsValue::String("1970-01-01T00:00:01.5Z".to_owned()))
            .expect("iso"),
        JsDate(1_500)
    );
    assert_eq!(
        cbor::from_value::<JsDate>(JsValue::Float(-1.9)).expect("ms"),
        JsDate(-1)
    );
}

#[test]
fn buffered_enums_read_undefined_as_none() {
    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(tag = "type", rename_all = "lowercase")]
    enum Event {
        Note {
            #[serde(default)]
            text: Option<String>,
        },
    }
    let value = JsValue::Object(IndexMap::from([
        ("type".to_owned(), JsValue::String("note".to_owned())),
        ("text".to_owned(), JsValue::Undefined),
    ]));
    assert_eq!(
        cbor::from_value::<Event>(value).expect("decodes"),
        Event::Note { text: None }
    );
}
