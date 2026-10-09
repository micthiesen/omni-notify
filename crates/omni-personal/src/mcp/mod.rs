//! MCP tools owned by this package (`src/mcp/tools/{personal,printer,browser-history}.ts`),
//! in the TS registration order. Metadata comes from the golden tool list.

use serde::Serialize;
use serde_json::Value;

use omni_mcp_kit::ToolError;

pub mod browser_history;
pub mod personal;
pub mod printer;

/// Serializes tool output with JS number formatting (`12`, not `12.0`).
pub(crate) fn js_output<T: Serialize>(value: &T) -> Result<Value, ToolError> {
    serde_json::to_value(value)
        .map(crate::js::normalize_numbers)
        .map_err(|e| ToolError::output(e.to_string()))
}
