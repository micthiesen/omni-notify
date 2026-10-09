//! Golden compatibility with the TypeScript implementation. Fixtures come from
//! `node --import tsx crates/omni-reminders/scripts/golden.mjs` (TS store, codec,
//! SRP, recurrence and tough-cookie code).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::type_complexity)]

use std::os::unix::fs::PermissionsExt as _;

use num_bigint::BigUint;
use omni_reminders::apple::srp::{GsaSrpAuthenticator, ServerChallenge, SrpProtocol};
use omni_reminders::codec::{crdt_protobuf, decode_crdt_document};
use omni_reminders::cookies::CookieJar;
use omni_reminders::recurrence::{RecurrenceEncoded, encode_recurrence_values};
use omni_reminders::service::fingerprint;
use omni_reminders::store::{FileRemindersStore, OperationState, RemindersStore as _};
use serde_json::Value;

fn golden(name: &str) -> Value {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn b64(s: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

#[tokio::test]
async fn decrypts_a_store_written_by_typescript_and_rewrites_it_readably() {
    let fixture = golden("store.json");
    let dir = tempfile::tempdir().unwrap();
    let private = dir.path().join("reminders-private");
    std::fs::create_dir(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = private.join(fixture["filename"].as_str().unwrap());
    std::fs::write(&file, b64(fixture["ciphertextBase64"].as_str().unwrap())).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let store = FileRemindersStore::new(
        &private,
        fixture["key"].as_str().unwrap(),
        fixture["account"].as_str().unwrap(),
    );
    assert_eq!(store.path(), file);
    let state = store.read().await.unwrap();
    assert_eq!(serde_json::to_value(&state).unwrap(), fixture["state"]);
    assert!(state.notified);
    let states: Vec<_> = state.operations.values().map(|o| o.state).collect();
    assert_eq!(
        states,
        [OperationState::Confirmed, OperationState::Reserved]
    );

    // The account identity is bound into the AAD: a different account cannot read it.
    let other = FileRemindersStore::new(
        &private,
        fixture["key"].as_str().unwrap(),
        "other@example.com",
    );
    std::fs::copy(&file, other.path()).unwrap();
    std::fs::set_permissions(other.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(other.read().await.is_err());

    // A Rust rewrite decodes to the same value (rollback safety, round trip).
    store.write(state.clone()).await.unwrap();
    assert_eq!(store.read().await.unwrap(), state);
}

#[test]
fn round_trips_the_tough_cookie_jar_written_by_typescript() {
    let fixture = golden("store.json");
    let serialized = fixture["state"]["session"]["cookies"].as_str().unwrap();
    let original: Value = serde_json::from_str(serialized).unwrap();
    let mut jar = CookieJar::from_json(&original).unwrap();
    assert_eq!(jar.to_json(), original, "serializeSync() output differs");
    // Matching follows tough-cookie (headers captured before the jar was serialized;
    // the creation order and path-length ordering are part of the fixture).
    let headers = fixture["cookieHeaders"].as_object().unwrap();
    let now = 1_800_000_000_000;
    for (url, expected) in headers {
        let url = url::Url::parse(url).unwrap();
        assert_eq!(
            &jar.cookie_header(&url, now),
            expected.as_str().unwrap(),
            "{url}"
        );
    }
}

#[test]
fn crdt_protobuf_matches_typescript_and_decodes_its_documents() {
    let fixture = golden("codec.json");
    for case in fixture["crdt"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        assert_eq!(
            crdt_protobuf(text).unwrap(),
            b64(case["protobufBase64"].as_str().unwrap()),
            "{text:?}"
        );
        assert_eq!(
            decode_crdt_document(case["encodedBase64"].as_str().unwrap()).unwrap(),
            text
        );
    }
}

#[test]
fn srp_proofs_match_typescript_for_both_protocols() {
    let fixture = golden("codec.json");
    for case in fixture["srp"].as_array().unwrap() {
        let a = BigUint::parse_bytes(case["a"].as_str().unwrap().as_bytes(), 16).unwrap();
        let auth = GsaSrpAuthenticator::with_secret(case["account"].as_str().unwrap(), a);
        assert_eq!(auth.public_a(), case["A"].as_str().unwrap());
        let server = &case["server"];
        let challenge = ServerChallenge {
            protocol: match server["protocol"].as_str().unwrap() {
                "s2k" => SrpProtocol::S2k,
                _ => SrpProtocol::S2kFo,
            },
            iteration: u32::try_from(server["iteration"].as_u64().unwrap()).unwrap(),
            salt: server["salt"].as_str().unwrap().into(),
            b: server["b"].as_str().unwrap().into(),
            c: server["c"].as_str().unwrap().into(),
        };
        let proof = auth
            .complete(case["password"].as_str().unwrap(), &challenge)
            .unwrap();
        assert_eq!(proof.m1, case["proof"]["m1"].as_str().unwrap());
        assert_eq!(proof.m2, case["proof"]["m2"].as_str().unwrap());
        assert_eq!(proof.c, "opaque-c");
        assert_eq!(
            proof.account_name,
            case["proof"]["accountName"].as_str().unwrap()
        );
    }
}

#[test]
fn recurrence_values_match_typescript() {
    let fixture = golden("codec.json");
    for case in fixture["recurrence"].as_array().unwrap() {
        let RecurrenceEncoded::Supported(values) = encode_recurrence_values(&case["rule"]) else {
            panic!("unsupported");
        };
        assert_eq!(Value::Object(values), case["encoded"]["values"]);
    }
}

#[test]
fn ledger_fingerprints_match_typescript() {
    let fixture = golden("codec.json");
    for case in fixture["fingerprints"].as_array().unwrap() {
        assert_eq!(
            fingerprint(&case["input"]),
            case["fingerprint"].as_str().unwrap(),
            "{}",
            case["input"]
        );
    }
}
