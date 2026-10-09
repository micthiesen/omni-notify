//! MCP golden metadata: offline regeneration from the TS sources and the policy
//! inventory (`src/mcp/policy.ts` `serializePolicyInventory`).

use std::path::Path;

use anyhow::{Context, Result, bail};
use omni_mcp_kit::golden::golden_tools;
use serde_json::{Value, json};

use crate::{repo_root, run_node};

const GOLDEN_FILES: [&str; 2] = ["tools-list.json", "handshake.json"];

/// `serializePolicyInventory`: tools sorted by `localeCompare` on name, pretty JSON
/// with two-space indent and a trailing newline.
pub fn render_policy() -> Result<String> {
    let golden = golden_tools().context("loading golden MCP metadata")?;
    let mut metas: Vec<_> = golden.metas.iter().collect();
    metas.sort_by(|a, b| omni_core::js::locale_compare(&a.name, &b.name));
    let tools: Vec<Value> = metas
        .into_iter()
        .map(|meta| {
            json!({
                "name": meta.name,
                "title": meta.title,
                "description": meta.description,
                "annotations": meta.annotations,
                "sideEffects": meta.policy.side_effects,
                "cost": meta.policy.cost,
                "recommendedExecutorPolicy": meta.policy.recommended_policy,
            })
        })
        .collect();
    let inventory = json!({
        "schemaVersion": 1,
        "generatedFrom": "src/mcp tool definitions",
        "tools": tools,
    });
    Ok(format!(
        "{}\n",
        omni_core::js::json_stringify_pretty2(&inventory)
    ))
}

pub fn mcp_policy(check: bool) -> Result<()> {
    let path = repo_root().join("docs/mcp-policy.json");
    let rendered = render_policy()?;
    if check {
        let current = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if current != rendered {
            bail!(
                "docs/mcp-policy.json differs from the golden tool metadata; run `cargo xtask mcp-golden`"
            );
        }
        println!("mcp-policy: docs/mcp-policy.json matches");
        return Ok(());
    }
    std::fs::write(&path, rendered).with_context(|| format!("writing {}", path.display()))?;
    println!("mcp-policy: wrote {}", path.display());
    Ok(())
}

fn read_json(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Regenerates `crates/omni-mcp-kit/golden` from `src/mcp` with inert services. With
/// `check`, generates into a temporary directory and compares JSON values instead.
pub fn mcp_golden(check: bool) -> Result<()> {
    let root = repo_root();
    let golden_dir = root.join("crates/omni-mcp-kit/golden");
    let policy = root.join("docs/mcp-policy.json");
    if !check {
        run_node("mcp-golden.ts", &[&golden_dir.to_string_lossy()])?;
        std::fs::copy(&policy, golden_dir.join("mcp-policy.json"))
            .context("copying docs/mcp-policy.json")?;
        println!("mcp-golden: wrote {}", golden_dir.display());
        return Ok(());
    }
    let scratch = std::env::temp_dir().join(format!("omni-mcp-golden-{}", std::process::id()));
    run_node("mcp-golden.ts", &[&scratch.to_string_lossy()])?;
    let mut stale = Vec::new();
    for file in GOLDEN_FILES {
        if read_json(&scratch.join(file))? != read_json(&golden_dir.join(file))? {
            stale.push(file);
        }
    }
    if read_json(&policy)? != read_json(&golden_dir.join("mcp-policy.json"))? {
        stale.push("mcp-policy.json");
    }
    std::fs::remove_dir_all(&scratch).ok();
    if !stale.is_empty() {
        bail!("stale MCP golden files {stale:?}; run `cargo xtask mcp-golden`");
    }
    println!("mcp-golden: golden files match the TS sources");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_rendering_is_byte_identical_to_the_ts_generator() {
        let docs = std::fs::read_to_string(repo_root().join("docs/mcp-policy.json")).unwrap();
        assert_eq!(render_policy().unwrap(), docs);
    }
}
