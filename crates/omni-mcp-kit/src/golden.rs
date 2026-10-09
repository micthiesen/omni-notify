//! Golden MCP metadata embedded at build time.
//!
//! - `golden/tools-list.json`: the TS server's `tools/list` result (identical in the
//!   2025-11-25 and 2026-07-28 eras), generated offline by
//!   `cargo xtask mcp-golden` from `src/mcp` with inert services.
//! - `golden/handshake.json`: the legacy `initialize` result, the modern
//!   `server/discover` result and each era's `tools/list` envelope.
//! - `golden/mcp-policy.json`: a copy of `docs/mcp-policy.json` (`golden-check` keeps
//!   them equal).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{Annotations, ExecutorPolicy, ToolMeta, ToolMetaError, ToolPolicy};

pub const TOOLS_LIST_JSON: &str = include_str!("../golden/tools-list.json");
pub const HANDSHAKE_JSON: &str = include_str!("../golden/handshake.json");
pub const POLICY_JSON: &str = include_str!("../golden/mcp-policy.json");

#[derive(Deserialize)]
struct ToolsList {
    tools: Vec<Map<String, Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyInventory {
    tools: Vec<PolicyEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyEntry {
    name: String,
    side_effects: Vec<String>,
    cost: String,
    recommended_executor_policy: ExecutorPolicy,
}

/// Every golden tool in `tools/list` order, plus the raw JSON objects served verbatim.
pub struct GoldenTools {
    pub metas: Vec<ToolMeta>,
    pub raw: Vec<Map<String, Value>>,
    index: HashMap<String, usize>,
}

impl GoldenTools {
    pub fn get(&self, name: &str) -> Option<(&ToolMeta, &Map<String, Value>)> {
        self.index
            .get(name)
            .map(|&i| (&self.metas[i], &self.raw[i]))
    }
}

fn str_field(tool: &Map<String, Value>, key: &str) -> Result<String, ToolMetaError> {
    tool.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ToolMetaError::InvalidGolden(format!("tool without string {key:?}")))
}

fn object_field(
    tool: &Map<String, Value>,
    name: &str,
    key: &str,
) -> Result<Arc<Map<String, Value>>, ToolMetaError> {
    match tool.get(key) {
        Some(Value::Object(map)) => Ok(Arc::new(map.clone())),
        _ => Err(ToolMetaError::InvalidGolden(format!(
            "tool {name:?} has no object {key:?}"
        ))),
    }
}

fn parse() -> Result<GoldenTools, ToolMetaError> {
    let invalid = |e: serde_json::Error| ToolMetaError::InvalidGolden(e.to_string());
    let list: ToolsList = serde_json::from_str(TOOLS_LIST_JSON).map_err(invalid)?;
    let policy: PolicyInventory = serde_json::from_str(POLICY_JSON).map_err(invalid)?;
    let policies: HashMap<String, PolicyEntry> = policy
        .tools
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect();
    let mut metas = Vec::with_capacity(list.tools.len());
    let mut index = HashMap::new();
    for tool in &list.tools {
        let name = str_field(tool, "name")?;
        let policy = policies.get(&name).ok_or_else(|| {
            ToolMetaError::InvalidGolden(format!("tool {name:?} has no policy entry"))
        })?;
        let annotations: Annotations =
            serde_json::from_value(tool.get("annotations").cloned().unwrap_or(Value::Null))
                .map_err(invalid)?;
        if index.insert(name.clone(), metas.len()).is_some() {
            return Err(ToolMetaError::InvalidGolden(format!(
                "duplicate MCP tool name {name:?}"
            )));
        }
        metas.push(ToolMeta {
            title: str_field(tool, "title")?,
            description: str_field(tool, "description")?,
            input_schema: object_field(tool, &name, "inputSchema")?,
            output_schema: object_field(tool, &name, "outputSchema")?,
            annotations,
            policy: ToolPolicy {
                side_effects: policy.side_effects.clone(),
                cost: policy.cost.clone(),
                recommended_policy: policy.recommended_executor_policy,
            },
            name,
        });
    }
    if policies.len() != metas.len() {
        return Err(ToolMetaError::InvalidGolden(format!(
            "policy inventory lists {} tools, tools/list {}",
            policies.len(),
            metas.len()
        )));
    }
    Ok(GoldenTools {
        metas,
        raw: list.tools,
        index,
    })
}

/// The parsed golden tools (parsed once).
pub fn golden_tools() -> Result<&'static GoldenTools, ToolMetaError> {
    static GOLDEN: OnceLock<Result<GoldenTools, String>> = OnceLock::new();
    GOLDEN
        .get_or_init(|| parse().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| ToolMetaError::InvalidGolden(e.clone()))
}

/// The golden `handshake.json`.
pub fn handshake() -> Result<Value, ToolMetaError> {
    serde_json::from_str(HANDSHAKE_JSON).map_err(|e| ToolMetaError::InvalidGolden(e.to_string()))
}
