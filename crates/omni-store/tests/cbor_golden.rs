//! omni_store::cbor against node-cbor 10.0.12 through mitools' encodeDoc /
//! decodeDoc (`scripts/golden-cbor.mjs` -> `tests/golden/cbor.json`).
//!
//! For every encode case: the Rust encoding of the JS value equals node's
//! bytes, decoding those bytes yields the value node decodes, and re-encoding
//! equals node's re-encoding. Decode cases are raw inputs (non-minimal heads,
//! half floats, indefinite items, every converted tag, malformed data): Rust
//! must accept exactly what node accepts, decode to the same JS value and
//! re-encode to node's bytes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::LazyLock;

use indexmap::IndexMap;
use omni_store::cbor::{self, JsValue};
use omni_store::logs_gz;
use serde_json::{Value, json};

static GOLDEN: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("golden/cbor.json")).expect("golden cbor.json parses")
});

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn f64_bits(s: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(s, 16).expect("bits"))
}

/// The node-side description of a JS value, mirrored from `describe()`.
fn describe(v: &JsValue) -> Value {
    const SAFE: i128 = 9_007_199_254_740_991;
    let bits = |x: f64| format!("{:016x}", x.to_bits());
    match v {
        JsValue::Undefined => json!({"t": "undefined"}),
        JsValue::Null => json!({"t": "null"}),
        JsValue::Bool(b) => json!({"t": "bool", "v": b}),
        #[allow(clippy::cast_precision_loss)]
        JsValue::Int(n) if n.abs() <= SAFE => json!({"t": "num", "bits": bits(*n as f64)}),
        JsValue::Int(n) | JsValue::BigInt(n) => json!({"t": "bigint", "v": n.to_string()}),
        JsValue::Float(x) => json!({"t": "num", "bits": bits(*x)}),
        JsValue::String(s) => json!({"t": "str", "v": s}),
        JsValue::Bytes(b) => json!({"t": "bytes", "hex": hex(b)}),
        JsValue::Array(items) => {
            json!({"t": "arr", "items": items.iter().map(describe).collect::<Vec<_>>()})
        }
        JsValue::Object(map) => {
            // Object.keys order: canonical array indices first.
            let mut indexed: Vec<(u64, &String, &JsValue)> = Vec::new();
            let mut named = Vec::new();
            for (k, v) in map {
                let index = (!k.is_empty()
                    && k.len() <= 10
                    && (k == "0" || !k.starts_with('0'))
                    && k.bytes().all(|b| b.is_ascii_digit()))
                .then(|| k.parse::<u64>().ok())
                .flatten()
                .filter(|n| *n < u64::from(u32::MAX));
                match index {
                    Some(i) => indexed.push((i, k, v)),
                    None => named.push((k, v)),
                }
            }
            indexed.sort_by_key(|(i, _, _)| *i);
            let entries: Vec<Value> = indexed
                .into_iter()
                .map(|(_, k, v)| (k, v))
                .chain(named)
                .map(|(k, v)| json!([k, describe(v)]))
                .collect();
            json!({"t": "obj", "entries": entries})
        }
        JsValue::Map(entries) => json!({
            "t": "map",
            "entries": entries.iter().map(|(k, v)| json!([describe(k), describe(v)])).collect::<Vec<_>>()
        }),
        JsValue::Date(ms) => json!({"t": "date", "bits": bits(*ms)}),
        JsValue::Set(items) => {
            json!({"t": "set", "items": items.iter().map(describe).collect::<Vec<_>>()})
        }
        JsValue::Simple(n) => json!({"t": "simple", "v": n}),
        JsValue::Tagged(tag, inner)
            if typed_array_size(*tag).is_some_and(
                |size| matches!(&**inner, JsValue::Bytes(b) if b.len() % size == 0),
            ) =>
        {
            let JsValue::Bytes(b) = &**inner else {
                unreachable!()
            };
            json!({"t": "typed", "tag": tag, "hex": hex(b)})
        }
        JsValue::Tagged(tag, inner) => json!({"t": "tagged", "tag": tag, "v": describe(inner)}),
    }
}

/// Element size of the typed arrays node materializes (little-endian tags,
/// which is where a decoded big-endian array ends up).
fn typed_array_size(tag: u64) -> Option<usize> {
    match tag {
        64 | 68 | 72 => Some(1),
        69 | 77 => Some(2),
        70 | 78 | 85 => Some(4),
        71 | 79 | 86 => Some(8),
        _ => None,
    }
}

/// Builds the JsValue a node-side description stands for.
fn build(d: &Value) -> JsValue {
    let items = |key: &str| -> Vec<JsValue> {
        d[key]
            .as_array()
            .expect("items")
            .iter()
            .map(build)
            .collect()
    };
    match d["t"].as_str().expect("t") {
        "undefined" => JsValue::Undefined,
        "null" => JsValue::Null,
        "bool" => JsValue::Bool(d["v"].as_bool().expect("bool")),
        "num" => JsValue::Float(f64_bits(d["bits"].as_str().expect("bits"))),
        "bigint" => JsValue::BigInt(d["v"].as_str().expect("v").parse().expect("i128")),
        "str" => JsValue::String(d["v"].as_str().expect("str").to_owned()),
        "bytes" => JsValue::Bytes(unhex(d["hex"].as_str().expect("hex"))),
        "typed" => JsValue::Tagged(
            d["tag"].as_u64().expect("tag"),
            Box::new(JsValue::Bytes(unhex(d["hex"].as_str().expect("hex")))),
        ),
        "date" => JsValue::Date(f64_bits(d["bits"].as_str().expect("bits"))),
        "set" => JsValue::Set(items("items")),
        "arr" => JsValue::Array(items("items")),
        "map" => JsValue::Map(
            d["entries"]
                .as_array()
                .expect("entries")
                .iter()
                .map(|e| (build(&e[0]), build(&e[1])))
                .collect(),
        ),
        "obj" => JsValue::Object(
            d["entries"]
                .as_array()
                .expect("entries")
                .iter()
                .map(|e| (e[0].as_str().expect("key").to_owned(), build(&e[1])))
                .collect::<IndexMap<_, _>>(),
        ),
        "tagged" => JsValue::Tagged(d["tag"].as_u64().expect("tag"), Box::new(build(&d["v"]))),
        "simple" => JsValue::Simple(u8::try_from(d["v"].as_u64().expect("v")).expect("u8")),
        other => panic!("unknown description {other}"),
    }
}

fn cases(section: &str) -> &'static Vec<Value> {
    GOLDEN[section].as_array().expect("section")
}

#[test]
fn encode_vectors_match_node() {
    let mut failures = Vec::new();
    for case in cases("encode") {
        let name = case["name"].as_str().expect("name");
        let value = build(&case["value"]);
        let expected = case["hex"].as_str().expect("hex");
        if hex(&cbor::encode(&value)) != expected {
            failures.push(format!(
                "{name}: encode {} != node {expected}",
                hex(&cbor::encode(&value))
            ));
            continue;
        }
        let decoded = cbor::decode(&unhex(expected)).expect("node bytes decode");
        if describe(&decoded) != case["decoded"] {
            failures.push(format!(
                "{name}: decoded {} != node {}",
                describe(&decoded),
                case["decoded"]
            ));
        }
        let reencoded = hex(&cbor::encode(&decoded));
        if reencoded != case["reencoded"].as_str().expect("reencoded") {
            failures.push(format!(
                "{name}: re-encoded {reencoded} != node {}",
                case["reencoded"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(cases("encode").len() >= 100);
}

#[test]
fn decode_vectors_match_node() {
    let mut failures = Vec::new();
    for case in cases("decode") {
        let name = case["name"].as_str().expect("name");
        let input = unhex(case["hex"].as_str().expect("hex"));
        let result = cbor::decode(&input);
        match (case["ok"].as_bool().expect("ok"), result) {
            (true, Ok(decoded)) => {
                if describe(&decoded) != case["decoded"] {
                    failures.push(format!(
                        "{name}: decoded {} != node {}",
                        describe(&decoded),
                        case["decoded"]
                    ));
                }
                let reencoded = hex(&cbor::encode(&decoded));
                if reencoded != case["reencoded"].as_str().expect("reencoded") {
                    failures.push(format!(
                        "{name}: re-encoded {reencoded} != node {}",
                        case["reencoded"]
                    ));
                }
            }
            (true, Err(e)) => failures.push(format!("{name}: node decodes, Rust fails: {e}")),
            (false, Ok(v)) => failures.push(format!(
                "{name}: node fails ({}), Rust decodes {v:?}",
                case["error"]
            )),
            (false, Err(_)) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn deep_nesting_is_rejected_not_overflowed() {
    // Runs on a 1 MiB stack (half of a tokio worker's) in a debug build.
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            for opener in [0x81u8, 0x9f, 0xa1, 0xc1] {
                let mut deepest_ok = Vec::new();
                for _ in 0..cbor::MAX_DEPTH {
                    deepest_ok.push(opener);
                    if opener == 0xa1 {
                        deepest_ok.push(0x00);
                    }
                }
                deepest_ok.push(0x00);
                if opener == 0x9f {
                    deepest_ok.extend(std::iter::repeat_n(0xff, cbor::MAX_DEPTH));
                }
                assert!(
                    cbor::decode(&deepest_ok).is_ok(),
                    "{opener:#x} at MAX_DEPTH"
                );
                let mut too_deep = vec![opener; 100_000];
                too_deep.push(0x00);
                assert!(cbor::decode(&too_deep).is_err());
            }
        })
        .expect("spawn")
        .join()
        .expect("no stack overflow");
}

#[test]
fn node_logs_gz_decodes() {
    for case in cases("logsGz") {
        let lines = logs_gz::decode(case["gz"].as_str().expect("gz")).expect("decodes");
        let expected: Vec<logs_gz::LogLine> =
            serde_json::from_value(case["lines"].clone()).expect("lines");
        assert_eq!(lines, expected);
        // Rust output decodes back to the same lines.
        assert_eq!(
            logs_gz::decode(&logs_gz::encode(&lines).expect("encode")).expect("decode"),
            lines
        );
    }
}
