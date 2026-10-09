//! The registered tool set: golden-order listing, TS-compatible call results, and an
//! rmcp [`ServerHandler`] over them.
//!
//! WP12 owns the production MCP endpoint (bearer auth, activity recording, the events
//! pre-router); it builds a [`ToolRegistry`] from every subsystem's tools and either
//! serves [`RegistryServer`] or calls [`ToolRegistry::call`] from its own handler.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
    PaginatedRequestParams, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::golden::golden_tools;
use crate::{McpTool, ToolContext, ToolError, ToolMetaError, ToolOutput};

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum RegistryError {
    #[error("Duplicate MCP tool name: {0}")]
    Duplicate(String),
    #[error(
        "MCP tools differ from the golden tool list: missing {missing:?}, unexpected {unexpected:?}"
    )]
    GoldenMismatch {
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    #[error("invalid golden MCP metadata: {0}")]
    Golden(String),
}

impl From<ToolMetaError> for RegistryError {
    fn from(e: ToolMetaError) -> Self {
        RegistryError::Golden(e.to_string())
    }
}

/// Every registered tool, listed in golden order.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<Vec<McpTool>>,
    by_name: Arc<HashMap<String, usize>>,
    listed: Arc<Vec<Tool>>,
}

impl ToolRegistry {
    /// Rejects duplicate names (TS `Duplicate MCP tool name`). Tools are listed in
    /// golden order regardless of registration order.
    pub fn new(tools: Vec<McpTool>) -> Result<Self, RegistryError> {
        let golden = golden_tools()?;
        let mut seen = HashMap::new();
        for (i, tool) in tools.iter().enumerate() {
            if seen.insert(tool.meta.name.clone(), i).is_some() {
                return Err(RegistryError::Duplicate(tool.meta.name.clone()));
            }
        }
        let mut ordered: Vec<McpTool> = Vec::with_capacity(tools.len());
        let mut listed = Vec::with_capacity(tools.len());
        let mut remaining = tools;
        for (meta, raw) in golden.metas.iter().zip(&golden.raw) {
            if let Some(pos) = remaining.iter().position(|t| t.meta.name == meta.name) {
                ordered.push(remaining.swap_remove(pos));
                listed.push(rmcp_tool(raw)?);
            }
        }
        // Tools outside the golden list cannot exist (`golden_meta` fails first), but
        // keep any defensively at the end.
        for tool in remaining {
            listed.push(rmcp_tool(&golden_raw(&tool.meta.name)?)?);
            ordered.push(tool);
        }
        let by_name = ordered
            .iter()
            .enumerate()
            .map(|(i, tool)| (tool.meta.name.clone(), i))
            .collect();
        Ok(Self {
            tools: Arc::new(ordered),
            by_name: Arc::new(by_name),
            listed: Arc::new(listed),
        })
    }

    /// Boot check: the registered set must equal the golden tool list.
    pub fn verify_complete(&self) -> Result<(), RegistryError> {
        let golden = golden_tools()?;
        let missing: Vec<String> = golden
            .metas
            .iter()
            .filter(|meta| !self.by_name.contains_key(&meta.name))
            .map(|meta| meta.name.clone())
            .collect();
        let unexpected: Vec<String> = self
            .tools
            .iter()
            .filter(|tool| golden.get(&tool.meta.name).is_none())
            .map(|tool| tool.meta.name.clone())
            .collect();
        if missing.is_empty() && unexpected.is_empty() {
            Ok(())
        } else {
            Err(RegistryError::GoldenMismatch {
                missing,
                unexpected,
            })
        }
    }

    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    pub fn get(&self, name: &str) -> Option<&McpTool> {
        self.by_name.get(name).map(|&i| &self.tools[i])
    }

    /// The `tools/list` entries, exactly as the TS server lists them.
    pub fn listed(&self) -> &[Tool] {
        &self.listed
    }

    /// Runs a tool and formats the result like `successfulToolResult` /
    /// `formatResult` / `failedToolResult`. `None` when no such tool is registered.
    pub async fn call(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
        cx: ToolContext,
    ) -> Option<Result<ToolOutput, ToolError>> {
        let tool = self.get(name)?;
        let input = Value::Object(arguments.unwrap_or_default());
        Some(tool.handler.call(input, cx).await)
    }
}

fn golden_raw(name: &str) -> Result<Map<String, Value>, RegistryError> {
    golden_tools()?
        .get(name)
        .map(|(_, raw)| raw.clone())
        .ok_or_else(|| RegistryError::Golden(format!("no golden tool {name:?}")))
}

fn rmcp_tool(raw: &Map<String, Value>) -> Result<Tool, RegistryError> {
    serde_json::from_value(Value::Object(raw.clone()))
        .map_err(|e| RegistryError::Golden(format!("tool is not a valid rmcp Tool: {e}")))
}

/// `successfulToolResult` / `formatResult` / `failedToolResult`.
pub fn call_tool_result(result: Result<ToolOutput, ToolError>) -> CallToolResult {
    let value = match result {
        Ok(ToolOutput::Structured(structured)) => {
            let text = omni_core::js::json_stringify(&Value::Object(structured.clone()));
            serde_json::json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": structured,
            })
        }
        Ok(ToolOutput::Custom {
            structured,
            content,
        }) => serde_json::json!({
            "content": content,
            "structuredContent": structured,
        }),
        Err(error) => failed(&error.message),
    };
    serde_json::from_value(value)
        .unwrap_or_else(|e| fallback_failure(&format!("invalid tool result content: {e}")))
}

fn failed(message: &str) -> Value {
    serde_json::json!({
        "isError": true,
        "content": [{"type": "text", "text": message}],
    })
}

fn fallback_failure(message: &str) -> CallToolResult {
    let mut result = CallToolResult::error(vec![rmcp::model::ContentBlock::text(message)]);
    result.is_error = Some(true);
    result
}

/// An rmcp server over a [`ToolRegistry`] with the golden server info and instructions.
#[derive(Clone)]
pub struct RegistryServer {
    registry: ToolRegistry,
    info: ServerConfig,
}

impl RegistryServer {
    /// Uses the golden `initialize` result (server info, capabilities, instructions).
    pub fn new(registry: ToolRegistry) -> Result<Self, RegistryError> {
        let handshake = crate::golden::handshake()?;
        let initialize = handshake
            .pointer("/legacy/initialize")
            .cloned()
            .ok_or_else(|| RegistryError::Golden("handshake has no initialize".to_owned()))?;
        let info: ServerConfig = serde_json::from_value(initialize)
            .map_err(|e| RegistryError::Golden(format!("invalid initialize result: {e}")))?;
        Ok(Self { registry, info })
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }
}

impl ServerHandler for RegistryServer {
    fn get_info(&self) -> ServerConfig {
        self.info.clone()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(
            self.registry.listed().to_vec(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let cx = ToolContext {
            call_id: context.id.to_string(),
            cancel: context.ct.clone(),
        };
        match self
            .registry
            .call(&request.name, request.arguments, cx)
            .await
        {
            Some(result) => Ok(call_tool_result(result).into()),
            None => Err(McpError::invalid_params(
                format!("Tool {} not found", request.name),
                None,
            )),
        }
    }
}

/// A fresh per-call context (tests and non-rmcp callers).
pub fn standalone_context(call_id: impl Into<String>) -> ToolContext {
    ToolContext {
        call_id: call_id.into(),
        cancel: CancellationToken::new(),
    }
}
