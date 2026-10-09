//! Production-copy compatibility (ignored; run with `OMNI_PROD_COPY=<copy of
//! docstore.db>`). Every mail transport row must decode into its typed model and
//! re-encode to the same value (explicit `undefined` fields excepted).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use std::path::PathBuf;

use omni_imap::archive_store::{ArchiveAction, decode_action};
use omni_imap::compose::StoredAttempt;
use omni_imap::cursor::ImapFolderCursor;
use omni_store::DocOps as _;
use omni_store::cbor::{self, JsValue};
use omni_store::entity::EntityDescriptor;
use omni_testkit::TestStore;

fn strip_undefined(value: JsValue) -> JsValue {
    match value {
        JsValue::Object(map) => JsValue::Object(
            map.into_iter()
                .filter(|(_, v)| !matches!(v, JsValue::Undefined))
                .map(|(k, v)| (k, strip_undefined(v)))
                .collect(),
        ),
        JsValue::Array(items) => JsValue::Array(items.into_iter().map(strip_undefined).collect()),
        other => other,
    }
}

/// Order-insensitive object equality (re-encoded struct order may differ).
fn same(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Object(x), JsValue::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        (JsValue::Array(x), JsValue::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        (JsValue::Int(x), JsValue::Float(y)) | (JsValue::Float(y), JsValue::Int(x)) => {
            (*x as f64) == *y
        }
        _ => a == b,
    }
}

/// Persisted original MIME must satisfy the Sent-copy repair preconditions:
/// exact Message-ID, a Date and a From, parsed by the MIME parser.
fn check_prepared(pk: &str, attempt: &StoredAttempt, failures: &mut Vec<String>) {
    use base64::Engine as _;
    let (Some(prepared), Some(result)) = (&attempt.prepared, &attempt.result) else {
        return;
    };
    // The private copy is dropped once the Sent copy is verified.
    let Some(content) = &prepared.content else {
        return;
    };
    let Ok(content) = base64::engine::general_purpose::STANDARD.decode(content) else {
        failures.push(format!("{pk}: content is not base64"));
        return;
    };
    match omni_imap::mime::parse_message(&content, 0) {
        Ok(parsed) => {
            if parsed.message_id.as_deref() != Some(result.message_id.as_str())
                || parsed.date.is_none_or(|d| d == 0)
                || parsed.from.is_none()
            {
                failures.push(format!("{pk}: prepared MIME lacks exact identity headers"));
            }
        }
        Err(e) => failures.push(format!("{pk}: {e}")),
    }
}

#[tokio::test]
#[ignore = "needs OMNI_PROD_COPY"]
async fn prod_copy_rows_decode_and_round_trip() {
    let path = PathBuf::from(std::env::var("OMNI_PROD_COPY").expect("OMNI_PROD_COPY"));
    let store = TestStore::from_fixture(&path).await;
    let rows = store
        .store
        .read(|docs| {
            let mut out = Vec::new();
            for prefix in ["$imap-folder-cursor#", "email-archive:", "email-compose:"] {
                out.extend(docs.get_raw_rows_by_prefix(prefix)?);
            }
            Ok(out)
        })
        .await
        .unwrap();
    let cursor = EntityDescriptor::of::<ImapFolderCursor>();
    let mut counts = std::collections::BTreeMap::<String, (usize, usize)>::new();
    let mut failures = Vec::new();
    for row in rows {
        let entity = row.entity.clone().unwrap_or_default();
        let value = row.decode().unwrap();
        let reencoded: Result<JsValue, String> = match entity.as_str() {
            "imap-folder-cursor" => {
                let pk = (cursor.recompute_pk)(&value);
                if pk.as_deref() != Ok(row.pk.as_str()) {
                    failures.push(format!("{}: pk {:?}", row.pk, pk));
                }
                cbor::from_value::<ImapFolderCursor>(value.clone())
                    .map_err(|e| e.to_string())
                    .and_then(|v| cbor::to_value(&v).map_err(|e| e.to_string()))
            }
            "email-archive-action" => decode_action(&row.pk, value.clone())
                .map_err(|e| e.to_string())
                .and_then(|v: ArchiveAction| {
                    if omni_imap::archive_store::action_key(&v.action_id) != row.pk {
                        failures.push(format!("{}: key does not match actionId", row.pk));
                    }
                    cbor::to_value(&v).map_err(|e| e.to_string())
                }),
            "email-archive-message" => cbor::from_value::<String>(value.clone())
                .map_err(|e| e.to_string())
                .and_then(|v| cbor::to_value(&v).map_err(|e| e.to_string())),
            "email-archive-history" => cbor::from_value::<Vec<String>>(value.clone())
                .map_err(|e| e.to_string())
                .and_then(|v| cbor::to_value(&v).map_err(|e| e.to_string())),
            "email-compose-send" | "email-compose-draft" => {
                cbor::from_value::<StoredAttempt>(value.clone())
                    .map_err(|e| e.to_string())
                    .and_then(|v| {
                        check_prepared(&row.pk, &v, &mut failures);
                        cbor::to_value(&v).map_err(|e| e.to_string())
                    })
            }
            other => Err(format!("unexpected entity {other:?}")),
        };
        let entry = counts.entry(entity).or_default();
        entry.0 += 1;
        match reencoded {
            Ok(encoded) if same(&encoded, &strip_undefined(value.clone())) => entry.1 += 1,
            Ok(_) => failures.push(format!("{}: value changed on re-encode", row.pk)),
            Err(e) => failures.push(format!("{}: {e}", row.pk)),
        }
    }
    for (entity, (total, ok)) in &counts {
        println!("{entity}: {ok}/{total} rows decode and round-trip");
    }
    // UIDVALIDITY recovery reads omni-email's dispatch watermark by its fixed key.
    let watermark = omni_imap::cursor::last_dispatched_at(&store.store)
        .await
        .unwrap();
    println!("dispatch watermark readable: {}", watermark.is_some());
    assert!(watermark.is_some_and(|at| at > 0));
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(counts.values().map(|(t, _)| t).sum::<usize>() > 0);
}
