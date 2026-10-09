//! Shared wire pieces (WP00).

use serde::{Deserialize, Serialize};

/// Epoch milliseconds as JS numbers carry them.
pub type Ms = i64;

/// `{"error": "..."}`, the body of every JSON error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
}

impl ApiErrorBody {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

/// `{items, nextCursor | null, total}` pages (MCP-style offset cursors).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paginated<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<u64>,
    pub total: u64,
}

/// `encodeURIComponent` (kept here so the wasm frontend needs no `omni-core`).
pub fn encode_uri_component(s: &str) -> String {
    const UNRESERVED: &[u8] = b"-_.!~*'()";
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED.contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Path builders for the WP00-owned routes (the frontend uses these).
pub mod paths {
    use super::encode_uri_component;

    pub const TASKS: &str = "/api/tasks";
    pub const TASK_RUNS: &str = "/api/task-runs";
    pub const COSTS: &str = "/api/costs";
    pub const HEALTH: &str = "/api/health";

    /// `POST /api/tasks/:name/run`.
    pub fn task_run_now(name: &str) -> String {
        format!("/api/tasks/{}/run", encode_uri_component(name))
    }

    /// `GET /api/task-runs?task=&limit=`.
    pub fn task_runs(task: Option<&str>, limit: Option<u32>) -> String {
        let mut query = Vec::new();
        if let Some(task) = task {
            query.push(format!("task={}", encode_uri_component(task)));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        if query.is_empty() {
            TASK_RUNS.to_owned()
        } else {
            format!("{TASK_RUNS}?{}", query.join("&"))
        }
    }

    /// `GET /api/task-runs/:runId/logs`.
    pub fn run_logs(run_id: &str) -> String {
        format!("/api/task-runs/{}/logs", encode_uri_component(run_id))
    }

    /// `GET /api/task-runs/:runId/logs/stream` (SSE).
    pub fn run_logs_stream(run_id: &str) -> String {
        format!(
            "/api/task-runs/{}/logs/stream",
            encode_uri_component(run_id)
        )
    }

    /// `GET /api/costs?days=7|30|90|all`.
    pub fn costs(days: Option<u32>) -> String {
        match days {
            Some(days) => format!("{COSTS}?days={days}"),
            None => format!("{COSTS}?days=all"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_js() {
        assert_eq!(
            encode_uri_component("Live:Check é/?"),
            "Live%3ACheck%20%C3%A9%2F%3F"
        );
        assert_eq!(
            paths::run_logs("LiveCheckTask:abc"),
            "/api/task-runs/LiveCheckTask%3Aabc/logs"
        );
    }
}
