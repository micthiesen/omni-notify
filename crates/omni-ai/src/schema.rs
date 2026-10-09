//! Strict JSON schemas for structured output and tool arguments
//! (OpenAI `strict: true` rules).

use serde_json::{Map, Value};

/// The schemars schema of `T` made strict: every object has
/// `additionalProperties: false` and lists all properties as required;
/// properties that were optional become nullable.
pub fn strict_schema<T: schemars::JsonSchema>() -> Value {
    let mut value = schemars::schema_for!(T).to_value();
    if let Value::Object(root) = &mut value {
        root.remove("$schema");
    }
    make_strict(&mut value);
    value
}

/// Parses a structured-output response (AI SDK `NoObjectGeneratedError` on failure).
pub fn parse_object<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, crate::AiError> {
    if text.trim().is_empty() {
        return Err(crate::AiError::Schema(
            "No object generated: the model returned no text".to_owned(),
        ));
    }
    serde_json::from_str(text.trim())
        .map_err(|e| crate::AiError::Schema(format!("No object generated: {e}")))
}

/// Whether `schema` already satisfies OpenAI strict mode: every object lists all of
/// its properties as required and sets `additionalProperties: false`.
pub fn is_strict_compatible(schema: &Value) -> bool {
    match schema {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = map.get("properties") {
                let required: Vec<&str> = match map.get("required") {
                    Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
                    _ => Vec::new(),
                };
                if properties.keys().any(|k| !required.contains(&k.as_str()))
                    || map.get("additionalProperties") != Some(&Value::Bool(false))
                {
                    return false;
                }
            }
            map.values().all(is_strict_compatible)
        }
        Value::Array(items) => items.iter().all(is_strict_compatible),
        _ => true,
    }
}

fn make_strict(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let names: Option<Vec<String>> = match map.get("properties") {
                Some(Value::Object(properties)) => Some(properties.keys().cloned().collect()),
                _ => None,
            };
            if let Some(names) = names {
                let required: Vec<String> = match map.get("required") {
                    Some(Value::Array(items)) => items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                };
                if let Some(Value::Object(properties)) = map.get_mut("properties") {
                    for name in &names {
                        if !required.contains(name)
                            && let Some(schema) = properties.get_mut(name)
                        {
                            make_nullable(schema);
                        }
                    }
                }
                map.insert(
                    "required".to_owned(),
                    Value::Array(names.into_iter().map(Value::String).collect()),
                );
                map.insert("additionalProperties".to_owned(), Value::Bool(false));
            }
            for child in map.values_mut() {
                make_strict(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(make_strict),
        _ => {}
    }
}

fn make_nullable(schema: &mut Value) {
    let Value::Object(map) = schema else {
        return;
    };
    match map.get_mut("type") {
        Some(Value::String(kind)) if kind != "null" => {
            let kind = std::mem::take(kind);
            map.insert(
                "type".to_owned(),
                Value::Array(vec![Value::String(kind), Value::String("null".to_owned())]),
            );
        }
        Some(Value::Array(kinds)) => {
            if !kinds.iter().any(|k| k == "null") {
                kinds.push(Value::String("null".to_owned()));
            }
        }
        Some(_) => {}
        None => {
            let original = Value::Object(std::mem::take(map));
            let mut wrapper = Map::new();
            let mut null = Map::new();
            null.insert("type".to_owned(), Value::String("null".to_owned()));
            wrapper.insert(
                "anyOf".to_owned(),
                Value::Array(vec![original, Value::Object(null)]),
            );
            *map = wrapper;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct Example {
        name: String,
        #[serde(default)]
        note: Option<String>,
    }

    #[test]
    fn objects_are_closed_and_fully_required() {
        let schema = strict_schema::<Example>();
        assert_eq!(schema["additionalProperties"], Value::Bool(false));
        assert_eq!(schema["required"], serde_json::json!(["name", "note"]));
        assert_eq!(
            schema["properties"]["note"]["type"],
            serde_json::json!(["string", "null"])
        );
    }
}
