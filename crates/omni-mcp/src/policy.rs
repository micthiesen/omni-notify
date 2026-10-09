//! The Executor policy inventory: every registered tool
//! sorted by `localeCompare` on its name, serialized as `docs/mcp-policy.json`
//! (`JSON.stringify(inventory, null, 2)` plus a trailing newline).

use omni_mcp_kit::McpTool;
use serde_json::{Value, json};

pub const MCP_POLICY_SCHEMA_VERSION: u32 = 1;

pub fn build_policy_inventory(tools: &[McpTool]) -> Value {
    let mut sorted: Vec<&McpTool> = tools.iter().collect();
    sorted.sort_by(|a, b| omni_core::js::locale_compare(&a.meta.name, &b.meta.name));
    let tools: Vec<Value> = sorted
        .into_iter()
        .map(|tool| {
            let meta = tool.meta;
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
    json!({
        "schemaVersion": MCP_POLICY_SCHEMA_VERSION,
        "generatedFrom": "omni-mcp tool definitions",
        "tools": tools,
    })
}

pub fn serialize_policy_inventory(tools: &[McpTool]) -> String {
    format!(
        "{}\n",
        omni_core::js::json_stringify_pretty2(&build_policy_inventory(tools))
    )
}
