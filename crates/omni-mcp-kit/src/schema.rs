//! JSON Schemas for MCP tool inputs and outputs, derived from Rust types.
//!
//! Tool schemas are `#[derive(JsonSchema)]` types next to each tool's handler.
//! [`input_schema`] and [`output_schema`] run schemars (draft 2020-12, every
//! subschema inlined) and normalize the result into the shape MCP clients have
//! always been served (the JSON Schema dialect zod 4 emits), with its exact key
//! order, so `tools/list` stays byte-stable. The normalization rules:
//!
//! - Integers carry explicit bounds: a missing lower bound becomes
//!   `-(2^53 - 1)` and a missing upper bound `2^53 - 1`, unsigned
//!   types imply `minimum: 0`, and `#[schemars(transform = positive)]` turns the
//!   lower bound into `exclusiveMinimum: 0`. Number and integer `format`s are dropped.
//! - Nullability is explicit. An optional field (`Option<T>` in the deserialize
//!   contract, or `skip_serializing_if` in the serialize contract) is not nullable
//!   unless marked with [`nullable`]; a required field is nullable when its type is
//!   `Option<T>` (serialize contract) or it is marked [`nullable`]. Nullable schemas
//!   become `anyOf: [T, {"type": "null"}]`.
//! - `serde_json::Value` is `{}` and string-keyed maps are records with
//!   `propertyNames`.
//! - `required` follows property order. Unions are untagged enums (`anyOf`, or
//!   `oneOf` with [`one_of`]); literals are [`Lit`] fields with a [`Literal`].
//! - Keys are ordered `default`, description (leading when the schema has a
//!   default, is a union, or is marked [`lead_description`]), the type's keywords,
//!   then a trailing description.
//!
//! Anything outside this dialect (a `$ref`, an unknown keyword) is an error, so a
//! new construct must be handled here deliberately.

use std::borrow::Cow;

use schemars::generate::SchemaSettings;
use schemars::transform::Transform;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde_json::{Map, Value, json};

/// `Number.MAX_SAFE_INTEGER`, the implicit integer bound.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The JSON Schema dialect every tool schema declares.
pub const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// The served `format: email` pattern.
pub const EMAIL_PATTERN: &str = r"^(?!\.)(?!.*\.\.)([A-Za-z0-9_'+\-\.]*)[A-Za-z0-9_+-]@([A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$";

/// The served `format: uuid` pattern.
pub const UUID_PATTERN: &str = "^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}|00000000-0000-0000-0000-000000000000|ffffffff-ffff-ffff-ffff-ffffffffffff)$";

const DATE_BODY: &str = r"(?:(?:\d\d[2468][048]|\d\d[13579][26]|\d\d0[48]|[02468][048]00|[13579][26]00)-02-29|\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\d|30)|(?:02)-(?:0[1-9]|1\d|2[0-8])))";
const TIME_BODY: &str =
    r"T(?:(?:[01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d+)?)?(?:Z|([+-](?:[01]\d|2[0-3]):[0-5]\d)))";

const LEAD: &str = "x-omni-lead-description";
const NULLABLE: &str = "x-omni-nullable";
const NO_DEFAULT: &str = "x-omni-no-default";

/// Why a type's schema cannot be served.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct SchemaError {
    pub path: String,
    pub reason: String,
}

fn error(path: &str, reason: impl Into<String>) -> SchemaError {
    SchemaError {
        path: if path.is_empty() {
            "/".to_owned()
        } else {
            path.to_owned()
        },
        reason: reason.into(),
    }
}

/// The served input schema of `T` (deserialize contract).
pub fn input_schema<T: JsonSchema>() -> Result<Map<String, Value>, SchemaError> {
    let settings = settings().for_deserialize();
    root(
        settings.into_generator().into_root_schema_for::<T>(),
        Root::Input,
    )
}

/// The served output schema of `T` (serialize contract).
pub fn output_schema<T: JsonSchema>() -> Result<Map<String, Value>, SchemaError> {
    let settings = settings().for_serialize();
    root(
        settings.into_generator().into_root_schema_for::<T>(),
        Root::Output,
    )
}

fn settings() -> SchemaSettings {
    SchemaSettings::draft2020_12().with(|s| {
        s.inline_subschemas = true;
        s.meta_schema = None;
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Root {
    Input,
    Output,
}

fn root(schema: Schema, kind: Root) -> Result<Map<String, Value>, SchemaError> {
    let Value::Object(mut map) = normalize(schema.to_value(), "")? else {
        return Err(error("", "a tool schema must be an object schema"));
    };
    let union = map.contains_key("oneOf");
    if !union && map.get("type") != Some(&json!("object")) {
        return Err(error("", "a tool schema must have type object"));
    }
    map.shift_remove("type");
    let mut out = Map::new();
    if kind == Root::Output && !union {
        out.insert("$schema".into(), json!(DRAFT_2020_12));
        out.insert("type".into(), json!("object"));
    } else {
        out.insert("type".into(), json!("object"));
        out.insert("$schema".into(), json!(DRAFT_2020_12));
    }
    out.extend(map);
    Ok(out)
}

/// Marks a description as leading the schema's keywords instead of trailing
/// them.
pub fn lead_description(schema: &mut Schema) {
    schema.insert(LEAD.into(), true.into());
}

/// Allows `null`, keeping it even on an optional field. Use on
/// an `Option<T>` field that is also `required` (input) or `skip_serializing_if`
/// (output) when the wire value may be `null`.
pub fn nullable(schema: &mut Schema) {
    let Some(map) = schema.as_object_mut() else {
        return;
    };
    match map.get_mut("type") {
        Some(Value::String(kind)) if kind != "null" => {
            let kind = std::mem::take(kind);
            map.insert("type".into(), json!([kind, "null"]));
        }
        Some(Value::Array(kinds)) => {
            if !kinds.iter().any(|k| k == "null") {
                kinds.push(json!("null"));
            }
        }
        _ => {
            let already = map
                .get("anyOf")
                .and_then(Value::as_array)
                .is_some_and(|items| items.len() == 2 && items.iter().any(is_null_schema));
            if !already {
                let mut outer = Map::new();
                for key in META_KEYS {
                    if let Some(value) = map.shift_remove(key) {
                        outer.insert(key.into(), value);
                    }
                }
                let inner = std::mem::take(map);
                outer.insert("anyOf".into(), json!([inner, {"type": "null"}]));
                *map = outer;
            }
        }
    }
    map.insert(NULLABLE.into(), true.into());
}

/// Serves an untagged enum's alternatives as `oneOf` (discriminated unions).
pub fn one_of(schema: &mut Schema) {
    if let Some(map) = schema.as_object_mut()
        && let Some(alternatives) = map.shift_remove("anyOf")
    {
        map.insert("oneOf".into(), alternatives);
    }
}

/// Drops the `default` that a `#[serde(default)]` field would otherwise advertise
/// (the field is optional and the handler applies the default).
pub fn no_default(schema: &mut Schema) {
    schema.insert(NO_DEFAULT.into(), true.into());
}

/// A strictly positive integer: `exclusiveMinimum: 0`.
pub fn positive(schema: &mut Schema) {
    if let Some(map) = schema.as_object_mut() {
        map.shift_remove("minimum");
    }
    schema.insert("exclusiveMinimum".into(), 0.into());
}

/// A UUID string.
pub fn uuid(schema: &mut Schema) {
    schema.insert("format".into(), "uuid".into());
    schema.insert("pattern".into(), UUID_PATTERN.into());
}

/// An ISO `YYYY-MM-DD` date string.
pub fn date(schema: &mut Schema) {
    schema.insert("format".into(), "date".into());
    schema.insert("pattern".into(), format!("^{DATE_BODY}$").into());
}

/// An ISO date-time string with an offset.
pub fn date_time(schema: &mut Schema) {
    schema.insert("format".into(), "date-time".into());
    schema.insert("pattern".into(), format!("^{DATE_BODY}{TIME_BODY}$").into());
}

/// A literal: `{"type": <json type>, "const": value}`. Use as
/// `#[schemars(transform = Literal("trending"))]` on a [`Lit`] field.
#[derive(Clone, Copy, Debug)]
pub struct Literal<T>(pub T);

impl<T: Into<Value> + Clone> Transform for Literal<T> {
    fn transform(&mut self, schema: &mut Schema) {
        let value: Value = self.0.clone().into();
        let kind = match &value {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            _ => "string",
        };
        let description = schema
            .as_object_mut()
            .and_then(|m| m.shift_remove("description"));
        *schema = schemars::json_schema!({ "type": kind, "const": value });
        if let Some(description) = description {
            schema.insert("description".into(), description);
        }
    }
}

/// A placeholder field type whose schema a [`Literal`] transform supplies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lit;

impl JsonSchema for Lit {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Lit".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::default()
    }
}

/// Any JSON value: `{}`.
pub struct Unknown;

impl JsonSchema for Unknown {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Unknown".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::default()
    }
}

/// A union of number literals. Use as
/// `#[schemars(transform = NumberLiterals(&[7, 30, 90]))]`; a `default` and
/// `description` on the field stay outside the union.
#[derive(Clone, Copy, Debug)]
pub struct NumberLiterals(pub &'static [i64]);

impl Transform for NumberLiterals {
    fn transform(&mut self, schema: &mut Schema) {
        let mut out = Map::new();
        for key in ["default", "description"] {
            if let Some(value) = schema.get(key) {
                out.insert(key.into(), value.clone());
            }
        }
        let alternatives = self
            .0
            .iter()
            .map(|n| json!({"type": "number", "const": n}))
            .collect();
        out.insert("anyOf".into(), Value::Array(alternatives));
        *schema = Schema::from(out);
    }
}

fn type_of(map: &Map<String, Value>) -> Option<&str> {
    map.get("type").and_then(Value::as_str)
}

fn is_null_schema(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.len() == 1 && type_of(m) == Some("null"))
}

/// Removes the `null` alternative schemars adds for `Option<T>`.
fn strip_null(map: &mut Map<String, Value>) {
    if let Some(Value::Array(types)) = map.get_mut("type") {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let only = types.remove(0);
            map.insert("type".into(), only);
        }
        return;
    }
    let Some(Value::Array(any_of)) = map.get("anyOf") else {
        return;
    };
    if any_of.len() != 2 || !any_of.iter().any(is_null_schema) {
        return;
    }
    let Some(Value::Object(inner)) = any_of.iter().find(|v| !is_null_schema(v)).cloned() else {
        return;
    };
    map.shift_remove("anyOf");
    let outer = std::mem::take(map);
    map.extend(inner);
    for (key, value) in outer {
        map.insert(key, value);
    }
}

const META_KEYS: [&str; 2] = ["default", "description"];

fn normalize(value: Value, path: &str) -> Result<Value, SchemaError> {
    let mut map = match value {
        Value::Bool(true) => return Ok(Value::Object(Map::new())),
        Value::Object(map) => map,
        other => return Err(error(path, format!("unsupported schema {other}"))),
    };
    if map.contains_key("$ref") || map.contains_key("$defs") {
        return Err(error(
            path,
            "recursive or referenced schemas are not supported",
        ));
    }
    map.shift_remove("title");
    map.shift_remove("$schema");
    // `Option<Enum>` lists `null` among the values; nullability is `anyOf`.
    if let Some(Value::Array(values)) = map.get_mut("enum") {
        values.retain(|v| !v.is_null());
    }
    map.shift_remove(NULLABLE);
    let lead = map.shift_remove(LEAD).is_some();
    if map.shift_remove(NO_DEFAULT).is_some() || map.get("default") == Some(&Value::Null) {
        map.shift_remove("default");
    }

    // `type: [T, "null"]` becomes `anyOf: [T, null]`, metadata outside.
    if let Some(Value::Array(types)) = map.get("type") {
        let non_null: Vec<Value> = types.iter().filter(|t| *t != "null").cloned().collect();
        if non_null.len() != 1 || types.len() != 2 {
            return Err(error(path, format!("unsupported type list {types:?}")));
        }
        let mut outer = Map::new();
        for key in META_KEYS {
            if let Some(value) = map.shift_remove(key) {
                outer.insert(key.into(), value);
            }
        }
        map.insert("type".into(), non_null[0].clone());
        let inner = normalize(Value::Object(map), path)?;
        outer.insert("anyOf".into(), json!([inner, {"type": "null"}]));
        return Ok(Value::Object(order(outer, true)));
    }

    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(items)) = map.shift_remove(key) {
            let items = items
                .into_iter()
                .enumerate()
                .map(|(i, item)| normalize(item, &format!("{path}/{key}/{i}")))
                .collect::<Result<Vec<_>, _>>()?;
            map.insert(key.into(), Value::Array(items));
        }
    }

    match type_of(&map) {
        Some("integer") => {
            map.shift_remove("format");
            if map.contains_key("exclusiveMinimum") {
                map.shift_remove("minimum");
            } else if !map.contains_key("minimum") {
                map.insert("minimum".into(), json!(-MAX_SAFE_INTEGER));
            }
            if !map.contains_key("maximum") {
                map.insert("maximum".into(), json!(MAX_SAFE_INTEGER));
            }
        }
        Some("number") => {
            if matches!(
                map.get("format").and_then(Value::as_str),
                Some("double" | "float")
            ) {
                map.shift_remove("format");
            }
        }
        Some("array") => {
            if let Some(items) = map.shift_remove("items") {
                map.insert("items".into(), normalize(items, &format!("{path}/items"))?);
            }
        }
        Some("object") => object(&mut map, path)?,
        _ => {}
    }
    Ok(Value::Object(order(map, lead)))
}

fn object(map: &mut Map<String, Value>, path: &str) -> Result<(), SchemaError> {
    let required: Vec<String> = match map.shift_remove("required") {
        Some(Value::Array(names)) => names
            .into_iter()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    match map.shift_remove("properties") {
        Some(Value::Object(properties)) => {
            let mut out = Map::new();
            for (name, schema) in properties {
                let mut schema = match schema {
                    Value::Object(m) => m,
                    other => {
                        out.insert(name.clone(), normalize(other, &format!("{path}/{name}"))?);
                        continue;
                    }
                };
                let keep_null = schema.contains_key(NULLABLE);
                if !required.contains(&name) && !keep_null {
                    strip_null(&mut schema);
                }
                let value = normalize(Value::Object(schema), &format!("{path}/{name}"))?;
                out.insert(name, value);
            }
            let ordered: Vec<Value> = out
                .keys()
                .filter(|k| required.contains(k))
                .map(|k| json!(k))
                .collect();
            if ordered.len() != required.len() {
                return Err(error(path, "required names a missing property"));
            }
            map.insert("properties".into(), Value::Object(out));
            if !ordered.is_empty() {
                map.insert("required".into(), Value::Array(ordered));
            }
        }
        Some(other) => return Err(error(path, format!("unsupported properties {other}"))),
        None => {
            if !required.is_empty() {
                return Err(error(path, "required without properties"));
            }
            match map.shift_remove("additionalProperties") {
                Some(Value::Bool(false)) => {
                    map.insert("properties".into(), json!({}));
                    map.insert("additionalProperties".into(), json!(false));
                }
                Some(values) => {
                    map.insert("propertyNames".into(), json!({"type": "string"}));
                    let values = normalize(values, &format!("{path}/additionalProperties"))?;
                    map.insert("additionalProperties".into(), values);
                }
                None => {
                    map.insert("propertyNames".into(), json!({"type": "string"}));
                    map.insert("additionalProperties".into(), json!({}));
                }
            }
        }
    }
    Ok(())
}

/// The served key order (see the module docs).
fn order(mut map: Map<String, Value>, lead: bool) -> Map<String, Value> {
    const ARRAY: [&str; 4] = ["minItems", "maxItems", "type", "items"];
    const OTHER: [&str; 17] = [
        "type",
        "minLength",
        "maxLength",
        "format",
        "pattern",
        "exclusiveMinimum",
        "minimum",
        "maximum",
        "enum",
        "const",
        "properties",
        "required",
        "propertyNames",
        "additionalProperties",
        "items",
        "anyOf",
        "oneOf",
    ];
    fn take(key: &str, from: &mut Map<String, Value>, out: &mut Map<String, Value>) {
        if let Some(value) = from.shift_remove(key) {
            out.insert(key.to_owned(), value);
        }
    }
    let mut out = Map::new();
    take("default", &mut map, &mut out);
    let leads = lead
        || out.contains_key("default")
        || map.contains_key("anyOf")
        || map.contains_key("oneOf");
    if leads {
        take("description", &mut map, &mut out);
    }
    let keys: &[&str] = if type_of(&map) == Some("array") {
        &ARRAY
    } else {
        &OTHER
    };
    for key in keys {
        take(key, &mut map, &mut out);
    }
    take("description", &mut map, &mut out);
    // Anything left is outside the dialect; keep it visible at the end.
    out.extend(map);
    out
}

/// Every key the dialect allows; [`check_dialect`] rejects any other.
const KNOWN: [&str; 21] = [
    "default",
    "description",
    "type",
    "minLength",
    "maxLength",
    "format",
    "pattern",
    "exclusiveMinimum",
    "minimum",
    "maximum",
    "enum",
    "const",
    "properties",
    "required",
    "propertyNames",
    "additionalProperties",
    "items",
    "anyOf",
    "oneOf",
    "minItems",
    "maxItems",
];

/// Fails on any keyword outside the served dialect (schemars extensions, markers
/// left on a schema the normalizer never visited).
pub fn check_dialect(schema: &Map<String, Value>) -> Result<(), SchemaError> {
    fn walk(value: &Value, path: &str) -> Result<(), SchemaError> {
        let Value::Object(map) = value else {
            return Ok(());
        };
        for (key, child) in map {
            if path.is_empty() && key == "$schema" {
                continue;
            }
            if !KNOWN.contains(&key.as_str()) {
                return Err(error(path, format!("unsupported keyword {key:?}")));
            }
            let at = format!("{path}/{key}");
            match key.as_str() {
                "properties" => {
                    if let Value::Object(properties) = child {
                        for (name, schema) in properties {
                            walk(schema, &format!("{at}/{name}"))?;
                        }
                    }
                }
                "anyOf" | "oneOf" => {
                    if let Value::Array(items) = child {
                        for (i, item) in items.iter().enumerate() {
                            walk(item, &format!("{at}/{i}"))?;
                        }
                    }
                }
                "items" | "additionalProperties" | "propertyNames" => walk(child, &at)?,
                _ => {}
            }
        }
        Ok(())
    }
    walk(&Value::Object(schema.clone()), "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(map: &Map<String, Value>) -> String {
        omni_core::js::json_stringify(&Value::Object(map.clone()))
    }

    #[derive(JsonSchema)]
    #[schemars(rename_all = "camelCase", deny_unknown_fields)]
    #[allow(dead_code)]
    struct Input {
        #[schemars(description = "Offset", extend("default" = 0))]
        cursor: Option<u64>,
        #[schemars(required, transform = nullable)]
        list_id: Option<String>,
        #[schemars(transform = nullable)]
        note: Option<String>,
        #[schemars(description = "Lead", transform = lead_description)]
        filter: Option<String>,
        #[schemars(description = "Trail", range(min = -5))]
        offset: i64,
        #[schemars(transform = positive)]
        id: u64,
        ratio: f64,
        tags: std::collections::BTreeMap<String, f64>,
        extra: Map<String, Value>,
        #[schemars(transform = date_time)]
        since: Option<String>,
        #[schemars(transform = NumberLiterals(&[7, 30]), extend("default" = 30))]
        days: Option<Lit>,
        kind: Option<Kind>,
    }

    #[derive(JsonSchema)]
    #[schemars(rename_all = "lowercase")]
    #[allow(dead_code)]
    enum Kind {
        Alpha,
        Beta,
    }

    #[test]
    fn inputs_follow_the_served_dialect() {
        let schema = input_schema::<Input>().unwrap();
        check_dialect(&schema).unwrap();
        let date_time = format!("^{DATE_BODY}{TIME_BODY}$");
        let expected = json!({
            "type": "object",
            "$schema": DRAFT_2020_12,
            "properties": {
                "cursor": {"default": 0, "description": "Offset", "type": "integer",
                    "minimum": 0, "maximum": MAX_SAFE_INTEGER},
                "listId": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                "note": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                "filter": {"description": "Lead", "type": "string"},
                "offset": {"type": "integer", "minimum": -5, "maximum": MAX_SAFE_INTEGER,
                    "description": "Trail"},
                "id": {"type": "integer", "exclusiveMinimum": 0, "maximum": MAX_SAFE_INTEGER},
                "ratio": {"type": "number"},
                "tags": {"type": "object", "propertyNames": {"type": "string"},
                    "additionalProperties": {"type": "number"}},
                "extra": {"type": "object", "propertyNames": {"type": "string"},
                    "additionalProperties": {}},
                "since": {"type": "string", "format": "date-time", "pattern": date_time},
                "days": {"default": 30, "anyOf": [
                    {"type": "number", "const": 7}, {"type": "number", "const": 30}]},
                "kind": {"type": "string", "enum": ["alpha", "beta"]},
            },
            "required": ["listId", "offset", "id", "ratio", "tags", "extra"],
            "additionalProperties": false,
        });
        assert_eq!(text(&schema), omni_core::js::json_stringify(&expected));
    }

    #[derive(JsonSchema)]
    #[schemars(rename_all = "camelCase", deny_unknown_fields)]
    #[allow(dead_code)]
    struct Page {
        #[schemars(transform = Literal("items"))]
        resource: Lit,
        next_cursor: Option<u64>,
        #[schemars(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[schemars(skip_serializing_if = "Option::is_none", transform = nullable)]
        tag: Option<String>,
        #[schemars(length(max = 2))]
        items: Vec<Option<Kind>>,
    }

    #[derive(JsonSchema)]
    #[schemars(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Profile {
        #[schemars(transform = Literal(true))]
        ok: Lit,
    }

    #[derive(JsonSchema)]
    #[schemars(untagged, transform = one_of)]
    #[allow(dead_code)]
    enum Output {
        Profile(Profile),
        Page(Page),
    }

    #[test]
    fn outputs_follow_the_served_dialect() {
        let schema = output_schema::<Page>().unwrap();
        let expected = json!({
            "$schema": DRAFT_2020_12,
            "type": "object",
            "properties": {
                "resource": {"type": "string", "const": "items"},
                "nextCursor": {"anyOf": [
                    {"type": "integer", "minimum": 0, "maximum": MAX_SAFE_INTEGER},
                    {"type": "null"}]},
                "error": {"type": "string"},
                "tag": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                "items": {"maxItems": 2, "type": "array", "items": {"anyOf": [
                    {"type": "string", "enum": ["alpha", "beta"]}, {"type": "null"}]}},
            },
            "required": ["resource", "nextCursor", "items"],
            "additionalProperties": false,
        });
        assert_eq!(text(&schema), omni_core::js::json_stringify(&expected));

        let union = output_schema::<Output>().unwrap();
        let keys: Vec<&str> = union.keys().map(String::as_str).collect();
        assert_eq!(keys, ["type", "$schema", "oneOf"]);
        assert_eq!(
            union["oneOf"][0],
            json!({"type": "object", "properties": {"ok": {"type": "boolean", "const": true}},
                "required": ["ok"], "additionalProperties": false})
        );
    }

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct Recursive {
        children: Vec<Recursive>,
    }

    #[test]
    fn constructs_outside_the_dialect_fail() {
        assert!(output_schema::<Recursive>().is_err());
        let mut odd = Map::new();
        odd.insert("type".into(), json!("string"));
        odd.insert("contentEncoding".into(), json!("base64"));
        assert!(check_dialect(&odd).is_err());
    }
}
