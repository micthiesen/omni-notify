//! Manual check against a copy of the production docstore (never the original):
//! `OMNI_PROD_COPY=/path/to/copy.db cargo test -p omni-briefings --test prod_copy -- --ignored --nocapture`.
//! Every briefing row must decode into its typed entity,
//! recompute its primary key, and re-encode to the same JS value once fields
//! stored as `undefined` are dropped (node reads absent and `undefined` alike).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use omni_store::cbor::{self, JsValue};
use omni_store::entity::{Entity, EntityDescriptor};
use omni_store::{DocOps, StoreError};
use omni_testkit::TestStore;

fn strip_undefined(value: JsValue) -> JsValue {
    match value {
        JsValue::Object(map) => JsValue::Object(
            map.into_iter()
                .filter(|(_, v)| *v != JsValue::Undefined)
                .map(|(k, v)| (k, strip_undefined(v)))
                .collect(),
        ),
        JsValue::Array(items) => JsValue::Array(items.into_iter().map(strip_undefined).collect()),
        other => other,
    }
}

#[derive(Debug, Default)]
struct Report {
    rows: usize,
    decoded: usize,
    pk_ok: usize,
    value_equal: usize,
    byte_identical: usize,
}

fn audit<E: Entity>(docs: &omni_store::Docs<'_>) -> Result<Report, StoreError> {
    let descriptor = EntityDescriptor::of::<E>();
    let mut report = Report::default();
    for (pk, value) in docs.get_docs_by_entity(E::NAME)? {
        report.rows += 1;
        let Ok(typed) = cbor::from_value::<E>(value.clone()) else {
            println!("  {pk}: typed decode failed");
            continue;
        };
        report.decoded += 1;
        if (descriptor.recompute_pk)(&value).as_deref() == Ok(pk.as_str()) {
            report.pk_ok += 1;
        }
        let encoded = cbor::encode(&cbor::to_value(&typed).unwrap());
        let reread = cbor::decode(&encoded).unwrap();
        if reread == strip_undefined(value.clone()) {
            report.value_equal += 1;
        } else {
            println!("  {pk}: re-encoded value differs");
        }
        if docs.get_raw_row(&pk)?.and_then(|r| r.data).as_deref() == Some(encoded.as_slice()) {
            report.byte_identical += 1;
        }
    }
    Ok(report)
}

#[tokio::test]
#[ignore = "needs OMNI_PROD_COPY pointing at a copy of the production docstore"]
async fn production_briefing_rows_decode_and_round_trip() {
    use omni_briefings::{BriefingDeliveryData, BriefingHistoryData};

    let source = std::env::var("OMNI_PROD_COPY").expect("OMNI_PROD_COPY");
    let copy = TestStore::from_fixture(std::path::Path::new(&source)).await;
    let reports = copy
        .store
        .read(|docs| {
            Ok(vec![
                ("briefing-history", audit::<BriefingHistoryData>(docs)?),
                ("briefing-delivery", audit::<BriefingDeliveryData>(docs)?),
            ])
        })
        .await
        .unwrap();
    for (name, r) in &reports {
        println!(
            "{name}: rows {}, decoded {}, pk {}, value-equal {}, byte-identical {}",
            r.rows, r.decoded, r.pk_ok, r.value_equal, r.byte_identical
        );
        assert!(r.rows > 0, "{name}: the copy has rows");
        assert_eq!(r.decoded, r.rows, "{name}: every row decodes");
        assert_eq!(r.pk_ok, r.rows, "{name}: every key recomputes");
        assert_eq!(
            r.value_equal, r.rows,
            "{name}: every row re-encodes to the same JS value"
        );
    }
    let summaries = omni_briefings::reader::briefing_summaries(&copy.store)
        .await
        .unwrap();
    println!("briefings served: {}", summaries.len());
    assert_eq!(summaries.len(), reports[0].1.rows);
}
