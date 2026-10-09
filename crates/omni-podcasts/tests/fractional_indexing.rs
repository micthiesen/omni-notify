//! Committed golden vectors for `generateKeyBetween` captured from rocicorp
//! fractional-indexing 4.0.0.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_podcasts::castro::fractional::generate_key_between;
use serde_json::Value;

fn golden() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/fractional_indexing.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn matches_every_pairwise_golden_case() {
    let golden = golden();
    let mut checked = 0;
    for case in golden["cases"].as_array().unwrap() {
        let a = case["a"].as_str();
        let b = case["b"].as_str();
        let actual = generate_key_between(a, b);
        match (
            case.get("key").and_then(Value::as_str),
            case.get("error").and_then(Value::as_str),
        ) {
            (Some(key), _) => assert_eq!(actual.as_deref(), Ok(key), "a={a:?} b={b:?}"),
            (None, Some(error)) => assert_eq!(
                actual.map_err(|e| e.0),
                Err(error.to_owned()),
                "a={a:?} b={b:?}"
            ),
            _ => panic!("malformed case {case}"),
        }
        checked += 1;
    }
    assert_eq!(checked, 1024);
}

#[test]
fn matches_append_prepend_and_bisect_chains() {
    let golden = golden();
    let mut key: Option<String> = None;
    for expected in golden["appends"].as_array().unwrap() {
        key = Some(generate_key_between(key.as_deref(), None).unwrap());
        assert_eq!(key.as_deref(), expected.as_str());
    }
    key = None;
    for expected in golden["prepends"].as_array().unwrap() {
        key = Some(generate_key_between(None, key.as_deref()).unwrap());
        assert_eq!(key.as_deref(), expected.as_str());
    }
    let mut lo = "a0".to_owned();
    for expected in golden["bisect"].as_array().unwrap() {
        lo = generate_key_between(Some(&lo), Some("a1")).unwrap();
        assert_eq!(Some(lo.as_str()), expected.as_str());
    }
}
