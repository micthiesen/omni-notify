//! The calendar extraction schema must be accepted by OpenAI strict mode.

use omni_ai::OutputSpec;
use omni_calendar::extraction::schema::CalendarEventExtraction;
use serde_json::Value;

fn keywords(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                found.push(key.clone());
                keywords(child, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keywords(item, found)),
        _ => {}
    }
}

#[test]
fn extraction_schema_is_openai_strict_compatible() {
    let schema = OutputSpec::of::<CalendarEventExtraction>().schema;
    assert!(!omni_ai::schema::has_ref_siblings(&schema), "{schema:#}");
    assert!(omni_ai::schema::is_strict_compatible(&schema), "{schema:#}");
    let mut found = Vec::new();
    keywords(&schema, &mut found);
    for unsupported in ["oneOf", "allOf", "not", "if", "patternProperties"] {
        assert!(
            !found.iter().any(|k| k == unsupported),
            "{unsupported} in {schema:#}"
        );
    }
}
