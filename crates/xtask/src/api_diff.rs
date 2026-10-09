//! `api-diff`: GET every read route from the TS and the Rust server (each on its own
//! copy of the database) and diff the JSON values, key presence and nulls included
//! (section 4.5 step 4). Non-JSON bodies are compared byte for byte.
//!
//! `--shape` compares JSON shapes instead of values, for servers whose databases
//! differ in content (a Rust shadow on an older copy against live TS): value kinds,
//! object keys, and for arrays the merged shape of all elements (lengths ignored).
//! Bodies of non-JSON responses are then compared by status only.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::capture::DEFAULT_ROUTES;
use crate::{flag_value, flag_values};

/// JSON-pointer paths where `a` and `b` differ (at most `limit`).
pub fn diff(a: &Value, b: &Value, path: &str, out: &mut Vec<String>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (key, value) in x {
                let child = format!("{path}/{key}");
                match y.get(key) {
                    Some(other) => diff(value, other, &child, out, limit),
                    None => out.push(format!("{child}: missing in rust")),
                }
            }
            for key in y.keys().filter(|k| !x.contains_key(*k)) {
                out.push(format!("{path}/{key}: extra in rust"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: length {} vs {}", x.len(), y.len()));
            }
            for (i, (value, other)) in x.iter().zip(y).enumerate() {
                diff(value, other, &format!("{path}/{i}"), out, limit);
            }
        }
        (Value::Number(x), Value::Number(y)) if x.as_f64() == y.as_f64() => {}
        _ if a == b => {}
        _ => out.push(format!("{path}: {a} vs {b}")),
    }
}

/// The JSON shape of a value: kinds and keys, with array elements merged.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Several kinds at one position (e.g. `null` and `string`).
    Union(Vec<Shape>),
    Null,
    Bool,
    Number,
    String,
    /// `None`: no element seen (empty array).
    Array(Option<Box<Shape>>),
    Object(std::collections::BTreeMap<String, Shape>),
}

impl Shape {
    pub fn of(value: &Value) -> Shape {
        match value {
            Value::Null => Shape::Null,
            Value::Bool(_) => Shape::Bool,
            Value::Number(_) => Shape::Number,
            Value::String(_) => Shape::String,
            Value::Array(items) => Shape::Array(
                items
                    .iter()
                    .map(Shape::of)
                    .reduce(Shape::merge)
                    .map(Box::new),
            ),
            Value::Object(map) => {
                Shape::Object(map.iter().map(|(k, v)| (k.clone(), Shape::of(v))).collect())
            }
        }
    }

    fn members(self) -> Vec<Shape> {
        match self {
            Shape::Union(members) => members,
            other => vec![other],
        }
    }

    /// Merges two shapes seen at the same position (array elements, union members).
    pub fn merge(self, other: Shape) -> Shape {
        let mut members = self.members();
        for shape in other.members() {
            let slot = members
                .iter()
                .position(|m| std::mem::discriminant(m) == std::mem::discriminant(&shape));
            match slot {
                Some(i) => {
                    let existing = members.remove(i);
                    members.insert(i, existing.merge_same(shape));
                }
                None => members.push(shape),
            }
        }
        if members.len() == 1 {
            members.remove(0)
        } else {
            Shape::Union(members)
        }
    }

    fn merge_same(self, other: Shape) -> Shape {
        match (self, other) {
            (Shape::Array(a), Shape::Array(b)) => Shape::Array(match (a, b) {
                (Some(a), Some(b)) => Some(Box::new(a.merge(*b))),
                (a, b) => a.or(b),
            }),
            (Shape::Object(mut a), Shape::Object(b)) => {
                for (key, shape) in b {
                    let merged = match a.remove(&key) {
                        Some(existing) => existing.merge(shape),
                        None => shape,
                    };
                    a.insert(key, merged);
                }
                Shape::Object(a)
            }
            (a, _) => a,
        }
    }

    fn kind(&self) -> String {
        match self {
            Shape::Union(members) => members
                .iter()
                .map(Shape::kind)
                .collect::<Vec<_>>()
                .join("|"),
            Shape::Null => "null".into(),
            Shape::Bool => "boolean".into(),
            Shape::Number => "number".into(),
            Shape::String => "string".into(),
            Shape::Array(_) => "array".into(),
            Shape::Object(_) => "object".into(),
        }
    }
}

/// Paths where the shapes differ. A position that is `null` on one side only
/// and a known kind on the other is reported (the nullability differs or the data
/// does not exercise it); empty arrays match any element shape.
pub fn shape_diff(a: &Shape, b: &Shape, path: &str, out: &mut Vec<String>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    match (a, b) {
        (Shape::Object(x), Shape::Object(y)) => {
            for (key, value) in x {
                let child = format!("{path}/{key}");
                match y.get(key) {
                    Some(other) => shape_diff(value, other, &child, out, limit),
                    None => out.push(format!("{child}: missing in rust")),
                }
            }
            for key in y.keys().filter(|k| !x.contains_key(*k)) {
                out.push(format!("{path}/{key}: extra in rust"));
            }
        }
        (Shape::Array(Some(x)), Shape::Array(Some(y))) => {
            shape_diff(x, y, &format!("{path}/[]"), out, limit);
        }
        (Shape::Array(_), Shape::Array(_)) => {}
        (Shape::Union(x), Shape::Union(y)) if x.len() == y.len() => {
            for m in x {
                let same = |n: &&Shape| std::mem::discriminant(m) == std::mem::discriminant(*n);
                match y.iter().find(same) {
                    Some(n) => shape_diff(m, n, path, out, limit),
                    None => {
                        out.push(format!("{path}: {} vs {}", a.kind(), b.kind()));
                        return;
                    }
                }
            }
        }
        _ if std::mem::discriminant(a) == std::mem::discriminant(b) => {}
        _ => out.push(format!("{path}: {} vs {}", a.kind(), b.kind())),
    }
}

pub fn api_diff(args: &[String]) -> Result<()> {
    let shape_only = args.iter().any(|a| a == "--shape");
    let ts = flag_value(args, "--ts").context("--ts URL is required")?;
    let rust = flag_value(args, "--rust").context("--rust URL is required")?;
    let mut routes: Vec<&str> = DEFAULT_ROUTES.to_vec();
    routes.extend(flag_values(args, "--route"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let differing = runtime.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent("OpenAI File Downloader, XaiImageApiFetch/1.0")
            .build()?;
        let mut differing = 0usize;
        for route in routes {
            let fetch = |base: &str| {
                let url = format!("{}{route}", base.trim_end_matches('/'));
                let client = client.clone();
                async move {
                    let response = client.get(&url).send().await?;
                    let status = response.status().as_u16();
                    let bytes = response.bytes().await?;
                    anyhow::Ok((status, bytes))
                }
            };
            let (ts_status, ts_body) = fetch(ts).await?;
            let (rust_status, rust_body) = fetch(rust).await?;
            let mut problems = Vec::new();
            if ts_status != rust_status {
                problems.push(format!("status {ts_status} vs {rust_status}"));
            }
            match (
                serde_json::from_slice::<Value>(&ts_body),
                serde_json::from_slice::<Value>(&rust_body),
            ) {
                (Ok(a), Ok(b)) if shape_only => {
                    shape_diff(&Shape::of(&a), &Shape::of(&b), "", &mut problems, 50);
                }
                (Ok(a), Ok(b)) => diff(&a, &b, "", &mut problems, 50),
                _ if shape_only || ts_body == rust_body => {}
                _ => problems.push("non-JSON bodies differ".to_owned()),
            }
            if problems.is_empty() {
                println!("same  {route}");
            } else {
                differing += 1;
                println!("DIFF  {route}");
                for problem in problems {
                    println!("      {problem}");
                }
            }
        }
        anyhow::Ok(differing)
    })?;
    if differing > 0 {
        bail!("{differing} routes differ");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn shape_diff_merges_array_elements_and_ignores_values() {
        let ts = json!({"items": [{"a": 1}, {"a": 2, "b": "x"}], "n": 3, "e": []});
        let rust = json!({"items": [{"a": 5, "b": "y"}], "n": 9, "e": [{"z": 1}]});
        let mut out = Vec::new();
        shape_diff(&Shape::of(&ts), &Shape::of(&rust), "", &mut out, 10);
        assert!(out.is_empty(), "{out:?}");

        let mixed_ts = json!([null, 1]);
        let mixed_rust = json!([2, null]);
        let mut out = Vec::new();
        shape_diff(
            &Shape::of(&mixed_ts),
            &Shape::of(&mixed_rust),
            "",
            &mut out,
            10,
        );
        assert!(out.is_empty(), "{out:?}");

        let rust = json!({"items": [{"a": "1", "c": 1}], "n": null});
        let mut out = Vec::new();
        shape_diff(&Shape::of(&ts), &Shape::of(&rust), "", &mut out, 10);
        assert_eq!(
            out,
            vec![
                "/e: missing in rust",
                "/items/[]/a: number vs string",
                "/items/[]/b: missing in rust",
                "/items/[]/c: extra in rust",
                "/n: number vs null",
            ]
        );
    }

    #[test]
    fn reports_missing_extra_and_changed_values() {
        let mut out = Vec::new();
        diff(
            &json!({"a": 1, "b": null, "c": [1, 2]}),
            &json!({"a": 1.0, "c": [1, 3], "d": true}),
            "",
            &mut out,
            10,
        );
        assert_eq!(
            out,
            vec!["/b: missing in rust", "/c/1: 2 vs 3", "/d: extra in rust"]
        );
    }
}
