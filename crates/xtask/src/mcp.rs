//! The MCP policy inventory rendered from the committed golden tool metadata
//! (`crates/omni-mcp-kit/golden`), which is the source of truth for every tool's
//! name, description, schemas and policy.

use anyhow::{Context, Result, bail};
use omni_mcp_kit::golden::golden_tools;
use serde_json::{Value, json};

use crate::repo_root;

/// The two copies of the inventory: the published one and the golden copy that
/// `omni-mcp-kit` embeds for policy metadata.
const POLICY_PATHS: [&str; 2] = [
    "docs/mcp-policy.json",
    "crates/omni-mcp-kit/golden/mcp-policy.json",
];

/// Tools sorted by `localeCompare` on name, pretty JSON with two-space indent and a
/// trailing newline.
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
        "generatedFrom": "omni-mcp tool definitions",
        "tools": tools,
    });
    Ok(format!(
        "{}\n",
        omni_core::js::json_stringify_pretty2(&inventory)
    ))
}

/// Writes both inventory copies, or with `check` fails when either differs.
pub fn mcp_policy(check: bool) -> Result<()> {
    let rendered = render_policy()?;
    let root = repo_root();
    let mut stale = Vec::new();
    for relative in POLICY_PATHS {
        let path = root.join(relative);
        if check {
            let current = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            if current != rendered {
                stale.push(relative);
            }
        } else {
            std::fs::write(&path, &rendered)
                .with_context(|| format!("writing {}", path.display()))?;
            println!("mcp-policy: wrote {relative}");
        }
    }
    if !stale.is_empty() {
        bail!("{stale:?} differ from the golden tool metadata; run `cargo xtask mcp-policy`");
    }
    if check {
        println!("mcp-policy: both inventory copies match");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_policy_copies_match_the_golden_tool_metadata() {
        let rendered = render_policy().unwrap();
        for relative in POLICY_PATHS {
            let current = std::fs::read_to_string(repo_root().join(relative)).unwrap();
            assert_eq!(rendered, current, "{relative}");
        }
    }
}
