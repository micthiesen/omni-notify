//! `events_status`: the MCP Events lifecycle Omni
//! has observed, without secrets, tokens, callback paths or message content.

use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolMetaError, typed_tool};
use serde::Deserialize;
use serde_json::{Value, json};

use super::conform;
use crate::events::service::McpEventService;

#[derive(Deserialize)]
struct EmptyInput {}

/// The status body; `None` reports MCP Events as disabled.
pub async fn events_status(events: Option<&McpEventService>) -> Result<Value, ToolError> {
    let value = match events {
        Some(events) => {
            let mut status = events
                .status()
                .await
                .map_err(|e| ToolError::execute_from(&e))?;
            status.insert("enabled".to_owned(), Value::Bool(true));
            Value::Object(status)
        }
        None => json!({
            "enabled": false,
            "checkedAt": null,
            "requests": [],
            "subscriptionTotal": 0,
            "subscriptions": [],
            "deliveries": {
                "pending": 0,
                "withheld": 0,
                "delivered": 0,
                "failed": 0,
                "recent": [],
            },
        }),
    };
    conform("events_status", value)
}

pub fn event_tools(events: Option<McpEventService>) -> Result<Vec<McpTool>, ToolMetaError> {
    let tool = typed_tool("events_status", move |_: EmptyInput, _: ToolContext| {
        let events = events.clone();
        async move { events_status(events.as_ref()).await }
    })?;
    Ok(vec![tool])
}
