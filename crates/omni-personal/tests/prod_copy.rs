//! Manual check against a copy of the production docstore (never the original):
//! `OMNI_PROD_COPY=/path/to/copy.db cargo test -p omni-personal --test prod_copy -- --ignored --nocapture`.
//!
//! Every `codex-reset-delivery`, `claude-reset-delivery` and `printer-accepted-job`
//! row must decode into its typed entity, recompute its primary key, and
//! re-encode to the same JS value; every `pets` / `pet_weight_history` row must
//! read through the typed tables.
#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use omni_personal::pets::persistence::PetStore;
use omni_personal::printer::AcceptedPrintRecord;
use omni_personal::reset_alerts::{ClaudeResetDelivery, CodexResetDelivery};
use omni_store::entity::{self, Entity};
use omni_store::{DocOps, Store, StoreError};
use omni_testkit::TestStore;

struct Report {
    rows: usize,
    decoded: usize,
    pk_matches: usize,
    value_equal: usize,
    byte_identical: usize,
}

async fn check<E: Entity>(store: &Store) -> Report {
    store
        .read(|docs| {
            let raw = docs.get_docs_by_entity(E::NAME)?;
            let mut report = Report {
                rows: raw.len(),
                decoded: 0,
                pk_matches: 0,
                value_equal: 0,
                byte_identical: 0,
            };
            for (pk, value) in &raw {
                let Ok(typed) = omni_store::cbor::from_value::<E>(value.clone()) else {
                    println!("{}: {pk} failed to decode", E::NAME);
                    continue;
                };
                report.decoded += 1;
                if entity::pk::<E>(&typed.key()).is_ok_and(|computed| computed == *pk) {
                    report.pk_matches += 1;
                }
                let encoded = omni_store::cbor::to_value(&typed)
                    .ok()
                    .map(|v| omni_store::cbor::encode(&v));
                let reread = encoded
                    .as_deref()
                    .and_then(|bytes| omni_store::cbor::decode(bytes).ok());
                if reread.as_ref() == Some(value) {
                    report.value_equal += 1;
                } else {
                    println!("{}: {pk} re-encodes to a different value", E::NAME);
                }
                let stored = docs.get_raw_row(pk)?.and_then(|row| row.data);
                if stored.is_some() && stored == encoded {
                    report.byte_identical += 1;
                }
            }
            Ok::<_, StoreError>(report)
        })
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs OMNI_PROD_COPY pointing at a copy of the production docstore"]
async fn production_rows_decode_and_round_trip() {
    let source = std::env::var("OMNI_PROD_COPY").unwrap();
    let copy = TestStore::from_fixture(std::path::Path::new(&source)).await;
    for (name, report) in [
        (
            "codex-reset-delivery",
            check::<CodexResetDelivery>(&copy.store).await,
        ),
        (
            "claude-reset-delivery",
            check::<ClaudeResetDelivery>(&copy.store).await,
        ),
        (
            "printer-accepted-job",
            check::<AcceptedPrintRecord>(&copy.store).await,
        ),
    ] {
        println!(
            "{name}: {} rows, {} decoded, {} pk matches, {} value-equal, {} byte-identical",
            report.rows,
            report.decoded,
            report.pk_matches,
            report.value_equal,
            report.byte_identical
        );
        assert_eq!(report.decoded, report.rows, "{name}");
        assert_eq!(report.pk_matches, report.rows, "{name}");
        assert_eq!(report.value_equal, report.rows, "{name}");
    }

    let pets = PetStore::open(&copy.store).await.unwrap();
    let all = pets.all_pets_with_history().await.unwrap();
    let readings: usize = all.iter().map(|p| p.weight_history.len()).sum();
    let raw_readings: i64 = copy
        .store
        .read(|docs| {
            docs.connection()
                .query_row("SELECT count(*) FROM pet_weight_history", [], |row| {
                    row.get(0)
                })
                .map_err(|e| StoreError::Sqlite(e.to_string()))
        })
        .await
        .unwrap();
    println!(
        "pets: {} pets, {readings} readings (table has {raw_readings})",
        all.len()
    );
    assert_eq!(readings as i64, raw_readings);
    for entry in &all {
        let visits = pets.daily_visit_counts(&entry.pet.pet_id).await.unwrap();
        let total: u32 = visits.iter().map(|v| v.count).sum();
        assert_eq!(total as usize, entry.weight_history.len());
    }
}
