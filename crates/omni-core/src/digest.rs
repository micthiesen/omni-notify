//! Content digests and constant-time comparison.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::js::{json_stringify, locale_compare};

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    let hash = Sha256::digest(bytes.as_ref());
    let mut out = String::with_capacity(64);
    for byte in hash.as_slice() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// SHA-256 hex truncated to 24 chars.
pub fn digest(s: &str) -> String {
    let mut hex = sha256_hex(s.as_bytes());
    hex.truncate(24);
    hex
}

/// Items in `localeCompare` order of their id, each
/// serialized as `JSON.stringify` of its top-level fields sorted by
/// `localeCompare`, joined by `\n`, then [`digest`].
///
/// Stored fingerprints omit absent fields, so callers model optional fields with
/// `#[serde(skip_serializing_if = "Option::is_none")]`; a serialized `null` is
/// kept.
///
/// Fails when an item does not serialize to a JSON object.
pub fn fingerprint_evidence<T: Serialize>(
    items: &[T],
    id: impl Fn(&T) -> &str,
) -> Result<String, serde_json::Error> {
    let mut ordered: Vec<&T> = items.iter().collect();
    ordered.sort_by(|a, b| locale_compare(id(a), id(b)));
    let mut lines = Vec::with_capacity(ordered.len());
    for item in ordered {
        let Value::Object(map) = serde_json::to_value(item)? else {
            return Err(serde::ser::Error::custom(
                "evidence item must serialize to a JSON object",
            ));
        };
        let mut fields: Vec<(String, Value)> = map.into_iter().collect();
        fields.sort_by(|(a, _), (b, _)| locale_compare(a, b));
        let sorted: serde_json::Map<String, Value> = fields.into_iter().collect();
        lines.push(json_stringify(&Value::Object(sorted)));
    }
    Ok(digest(&lines.join("\n")))
}

/// Hashes both inputs first so the comparison time reveals neither length nor content.
pub fn ct_eq_sha256(a: &[u8], b: &[u8]) -> bool {
    let left = Sha256::digest(a);
    let right = Sha256::digest(b);
    left.as_slice().ct_eq(right.as_slice()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(digest(""), "e3b0c44298fc1c149afbf4c8");
    }

    #[test]
    fn fingerprint_orders_items_and_keys() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Item {
            evidence_id: &'static str,
            b: u32,
            #[serde(skip_serializing_if = "Option::is_none")]
            a: Option<u32>,
        }
        let items = [
            Item {
                evidence_id: "z",
                b: 1,
                a: None,
            },
            Item {
                evidence_id: "y",
                b: 2,
                a: Some(3),
            },
        ];
        let expected =
            digest("{\"a\":3,\"b\":2,\"evidenceId\":\"y\"}\n{\"b\":1,\"evidenceId\":\"z\"}");
        assert_eq!(
            fingerprint_evidence(&items, |i| i.evidence_id).ok(),
            Some(expected)
        );
    }

    #[test]
    fn constant_time_equality() {
        assert!(ct_eq_sha256(b"token", b"token"));
        assert!(!ct_eq_sha256(b"token", b"tokens"));
    }
}
