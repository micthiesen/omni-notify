//! `api-diff`: GET every read route from the TS and the Rust server (each on its own
//! copy of the database) and diff the JSON values, key presence and nulls included
//! (section 4.5 step 4). Non-JSON bodies are compared byte for byte.

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

pub fn api_diff(args: &[String]) -> Result<()> {
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
                (Ok(a), Ok(b)) => diff(&a, &b, "", &mut problems, 50),
                _ if ts_body == rust_body => {}
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
