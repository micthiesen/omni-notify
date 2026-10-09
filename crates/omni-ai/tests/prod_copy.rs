//! Manual check against a copy of the production docstore (never the original):
//! `OMNI_PROD_COPY=/path/to/copy.db cargo test -p omni-ai --test prod_copy -- --ignored`.
//! Every `cost-event` and `cost-migration` row must decode into the typed entities, and
//! re-encoding a decoded event must reproduce the stored JS value. Byte identity is
//! reported only: legacy-import rows (eventId first) and transcription rows
//! (`usage: {requests, characters}`) use a different key order than the struct.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use omni_ai::costs::{CostEventData, CostMigrationData};
use omni_core::clock::SharedClock;
use omni_store::{DocOps, EntityOps};
use omni_testkit::{TEST_EPOCH_MS, TestStore, test_clock};

#[tokio::test]
#[ignore = "needs OMNI_PROD_COPY pointing at a copy of the production docstore"]
async fn production_cost_rows_decode_and_round_trip() {
    let source = std::env::var("OMNI_PROD_COPY").expect("OMNI_PROD_COPY");
    let _clock: SharedClock = test_clock(TEST_EPOCH_MS);
    let copy = TestStore::from_fixture(std::path::Path::new(&source)).await;
    let (raw, typed, migrations, identical, equal) =
        copy.store
            .read(|docs| {
                let raw = docs.get_docs_by_entity("cost-event")?;
                let typed = docs.get_all::<CostEventData>()?;
                let migrations = docs.get_all::<CostMigrationData>()?;
                let mut identical = 0usize;
                let mut equal = 0usize;
                for (pk, value) in &raw {
                    let event: CostEventData = omni_store::cbor::from_value(value.clone())
                        .map_err(|e| omni_store::StoreError::CorruptRow {
                            pk: pk.clone(),
                            reason: e.to_string(),
                        })?;
                    let stored = docs.get_raw_row(pk)?.and_then(|row| row.data);
                    let encoded = omni_store::cbor::to_value(&event)
                        .ok()
                        .map(|v| omni_store::cbor::encode(&v));
                    let reread = encoded
                        .as_deref()
                        .and_then(|bytes| omni_store::cbor::decode(bytes).ok());
                    if reread.as_ref() == Some(value) {
                        equal += 1;
                    }
                    if stored.is_some() && stored == encoded {
                        identical += 1;
                    }
                }
                Ok((raw.len(), typed.len(), migrations.len(), identical, equal))
            })
            .await
            .unwrap();
    println!(
        "cost-event rows {raw}, typed {typed}, value-equal {equal}, byte-identical {identical}"
    );
    assert!(raw > 0);
    assert_eq!(raw, typed, "every cost-event row decodes");
    assert_eq!(migrations, 1);
    assert_eq!(equal, raw, "every row re-encodes to the same JS value");
}
