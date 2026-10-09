//! Canonical digest binding a continuation to its tool name and arguments.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Key-sorted compact JSON. Only compared within one process, so it needs to be
/// deterministic, not byte-identical to any other rendering.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write(item, out);
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// SHA-256 over `{name, arguments ?? {}}`. A missing name is distinct from every
/// string name.
pub fn args_digest(params: &Map<String, Value>) -> String {
    let name = match params.get("name") {
        Some(value) => canonical(value),
        None => "undefined".to_owned(),
    };
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => "{}".to_owned(),
        Some(value) => canonical(value),
    };
    let rendered = format!("{{\"arguments\":{arguments},\"name\":{name}}}");
    hex::encode(Sha256::digest(rendered.as_bytes()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn digest_ignores_key_order_and_extra_fields() {
        let a = params(json!({"name": "execute", "arguments": {"b": 1, "a": [true, null]}}));
        let b = params(json!({
            "arguments": {"a": [true, null], "b": 1},
            "name": "execute",
            "requestState": "x",
            "inputResponses": {}
        }));
        assert_eq!(args_digest(&a), args_digest(&b));
    }

    #[test]
    fn digest_distinguishes_name_arguments_and_missing_name() {
        let base = args_digest(&params(json!({"name": "execute"})));
        assert_eq!(
            base,
            args_digest(&params(json!({"name": "execute", "arguments": null})))
        );
        assert_eq!(
            base,
            args_digest(&params(json!({"name": "execute", "arguments": {}})))
        );
        assert_ne!(base, args_digest(&params(json!({"name": "other"}))));
        assert_ne!(base, args_digest(&params(json!({"arguments": {}}))));
        assert_ne!(
            args_digest(&params(json!({"name": "undefined"}))),
            args_digest(&params(json!({})))
        );
    }

    #[test]
    fn canonical_sorts_nested_keys() {
        assert_eq!(
            canonical(&json!({"b": {"d": 1, "c": "x"}, "a": [2]})),
            r#"{"a":[2],"b":{"c":"x","d":1}}"#
        );
    }
}
