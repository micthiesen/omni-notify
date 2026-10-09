//! Property tests: every encodable value decodes, and decoding then
//! re-encoding is a fixed point (the byte-identity the production audit relies
//! on); entity keys never collide.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use indexmap::IndexMap;
use omni_store::cbor::{self, JsValue};
use omni_store::entity::{self, KeyPart};
use proptest::prelude::*;

fn leaf() -> impl Strategy<Value = JsValue> {
    prop_oneof![
        Just(JsValue::Undefined),
        Just(JsValue::Null),
        any::<bool>().prop_map(JsValue::Bool),
        any::<i64>().prop_map(|n| JsValue::Int(i128::from(n))),
        any::<u64>().prop_map(|n| JsValue::Int(i128::from(n))),
        any::<f64>().prop_map(JsValue::Float),
        any::<i128>().prop_map(JsValue::BigInt),
        ".{0,30}".prop_map(JsValue::String),
        proptest::collection::vec(any::<u8>(), 0..40).prop_map(JsValue::Bytes),
        // Whole seconds: `new Date(secs * 1000)` can drift by 1 ms for other
        // values in node as well, so only these are exact fixed points.
        #[allow(clippy::cast_precision_loss)]
        (-8_640_000_000_000i64..8_640_000_000_000).prop_map(|s| JsValue::Date((s * 1000) as f64)),
        (0u8..20).prop_map(JsValue::Simple),
        (32u8..=255).prop_map(JsValue::Simple),
    ]
}

fn value() -> impl Strategy<Value = JsValue> {
    leaf().prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(JsValue::Array),
            proptest::collection::vec(("[a-z0-9]{0,6}", inner.clone()), 0..8).prop_map(|entries| {
                JsValue::Object(entries.into_iter().collect::<IndexMap<_, _>>())
            }),
            proptest::collection::vec(inner.clone(), 0..4).prop_map(JsValue::Set),
            (1000u64..5000, inner).prop_map(|(tag, v)| JsValue::Tagged(tag, Box::new(v))),
        ]
    })
}

proptest! {
    #[test]
    fn decode_then_encode_is_a_fixed_point(v in value()) {
        let bytes = cbor::encode(&v);
        let decoded = cbor::decode(&bytes).expect("own output decodes");
        let again = cbor::encode(&decoded);
        prop_assert_eq!(&again, &cbor::encode(&cbor::decode(&again).expect("decodes")));
        let via_serde = cbor::to_value(&cbor::from_value::<JsValue>(decoded).expect("serde in")).expect("serde out");
        prop_assert_eq!(cbor::encode(&via_serde), again);
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = cbor::decode(&bytes);
    }

    #[test]
    fn string_keys_never_collide(a in ".{0,8}", b in ".{0,8}", c in ".{0,8}", d in ".{0,8}") {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Two { x: String, y: String }
        impl omni_store::Entity for Two {
            const NAME: &'static str = "two";
            type Key = (String, String);
            fn key(&self) -> Self::Key { (self.x.clone(), self.y.clone()) }
        }
        let k1 = entity::pk::<Two>(&(a.clone(), b.clone())).unwrap();
        let k2 = entity::pk::<Two>(&(c.clone(), d.clone())).unwrap();
        prop_assert_eq!(k1 == k2, (a.as_str(), b.as_str()) == (c.as_str(), d.as_str()));
        let prefix = entity::prefix::<Two>(&[KeyPart::from(a.as_str())]).unwrap();
        prop_assert!(k1.starts_with(&prefix));
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlattenedDoc {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<f64>,
    #[serde(flatten)]
    extra: cbor::Extra,
}

#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Tagged {
    Item { note: Option<String> },
}

/// TS writes explicit `undefined` properties; serde's buffered paths
/// (`#[serde(flatten)]`, internally tagged enums) read them as absent options,
/// and unknown `undefined`s survive a round trip as `undefined`, not `null`.
#[test]
fn explicit_undefined_reads_through_buffered_serde_paths() {
    let doc = JsValue::Object(IndexMap::from([
        ("id".to_owned(), JsValue::String("a".to_owned())),
        ("error".to_owned(), JsValue::Undefined),
        ("count".to_owned(), JsValue::Undefined),
        ("later".to_owned(), JsValue::Undefined),
        ("kept".to_owned(), JsValue::Null),
    ]));
    let decoded: FlattenedDoc = cbor::from_value(doc).expect("decodes");
    assert_eq!(decoded.error, None);
    assert_eq!(decoded.count, None);
    assert_eq!(decoded.extra.get("later"), Some(&JsValue::Undefined));
    assert_eq!(decoded.extra.get("kept"), Some(&JsValue::Null));
    let encoded = cbor::to_value(&decoded).expect("encodes");
    assert_eq!(encoded.get("later"), Some(&JsValue::Undefined));
    assert_eq!(encoded.get("kept"), Some(&JsValue::Null));
    assert_eq!(encoded.get("error"), None);

    let tagged = JsValue::Object(IndexMap::from([
        ("kind".to_owned(), JsValue::String("item".to_owned())),
        ("note".to_owned(), JsValue::Undefined),
    ]));
    assert_eq!(
        cbor::from_value::<Tagged>(tagged).expect("decodes"),
        Tagged::Item { note: None }
    );
    let bare: JsValue = cbor::from_value(JsValue::Undefined).expect("decodes");
    assert_eq!(bare, JsValue::Undefined);
}
