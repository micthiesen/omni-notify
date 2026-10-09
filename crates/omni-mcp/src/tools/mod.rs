//! The MCP tools this package owns (`src/mcp/tools/{system,events,claude-sessions}.ts`)
//! and the full-surface ordering check (`src/mcp/tools/index.ts`).

pub mod claude_sessions;
pub mod events;
pub mod system;

use futures::future::BoxFuture;
use omni_api::tasks::TaskInfo;
use omni_mcp_kit::{ToolError, golden_meta};
use omni_store::LogLine;
use omni_tasks::persistence::TaskRunData;
use omni_tasks::{RunNowError, TaskRegistry};
use serde_json::Value;

use crate::json::order_by_schema;

/// A run with its log lines and dropped-line count.
pub type RunWithLogs = (TaskRunData, Vec<LogLine>, u64);

/// The task registry as the system tools see it (a seam for tests).
pub trait TaskControl: Send + Sync {
    fn list(&self) -> BoxFuture<'_, Result<Vec<TaskInfo>, String>>;
    /// Queues a manual run; the error is the TS message.
    fn run_now(&self, name: &str, input: Option<Value>) -> Result<String, String>;
    fn recent_runs<'a>(
        &'a self,
        task: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TaskRunData>, String>>;
    /// A run with its log lines (live buffer first) and dropped count.
    fn run_logs<'a>(
        &'a self,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<RunWithLogs>, String>>;
}

impl TaskControl for TaskRegistry {
    fn list(&self) -> BoxFuture<'_, Result<Vec<TaskInfo>, String>> {
        Box::pin(async move { TaskRegistry::list(self).await.map_err(|e| e.to_string()) })
    }

    fn run_now(&self, name: &str, input: Option<Value>) -> Result<String, String> {
        TaskRegistry::run_now(self, name, input).map_err(|error| match error {
            RunNowError::NotFound => format!("Unknown task \"{name}\""),
            RunNowError::AlreadyRunning => format!("Task \"{name}\" is already running"),
            RunNowError::ManualInputUnsupported => {
                format!("Task \"{name}\" does not accept manual input")
            }
            other => other.to_string(),
        })
    }

    fn recent_runs<'a>(
        &'a self,
        task: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TaskRunData>, String>> {
        Box::pin(async move {
            TaskRegistry::recent_runs(self, task, limit)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn run_logs<'a>(
        &'a self,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<RunWithLogs>, String>> {
        Box::pin(async move {
            TaskRegistry::run_logs(self, run_id)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

/// Projects a value onto the tool's golden output schema (zod's output
/// parse): schema key order, unknown keys stripped.
pub(crate) fn conform(tool: &str, value: Value) -> Result<Value, ToolError> {
    let meta = golden_meta(tool).map_err(|e| ToolError::output(e.to_string()))?;
    Ok(order_by_schema(
        value,
        &Value::Object((*meta.output_schema).clone()),
    ))
}

/// `truncate()`: the first `max` UTF-16 units and whether anything was cut.
pub(crate) fn truncate(text: &str, max: usize) -> (String, bool) {
    omni_mcp_kit::truncate_utf16(text, max)
}
