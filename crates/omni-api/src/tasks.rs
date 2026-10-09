//! `/api/tasks` (WP00).

use serde::{Deserialize, Serialize};

use crate::runs::Run;

/// One registered task as the UI sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub schedule: String,
    pub running: bool,
    /// ISO strings of the next three fires (`getNextRuns(3)`).
    pub next_runs: Vec<String>,
    pub last_run: Option<Run>,
}

/// `GET /api/tasks` body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TasksResponse {
    pub tasks: Vec<TaskInfo>,
}

/// `POST /api/tasks/:name/run` success body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunNowResponse {
    pub run_id: String,
}
