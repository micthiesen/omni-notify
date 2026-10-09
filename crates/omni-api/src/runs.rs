//! Task runs and run logs: `/api/task-runs`, `/api/task-runs/:id/logs`
//! and the `init` / `line` / `done` frames of its SSE stream.

use serde::{Deserialize, Serialize};

use crate::common::Ms;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunTrigger {
    #[serde(rename = "schedule")]
    Schedule,
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "startup")]
    Startup,
    #[serde(rename = "catchup")]
    Catchup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Error,
    /// Completed but skipped its real work because an upstream failed;
    /// `error` holds the reason.
    #[serde(rename = "degraded")]
    Degraded,
}

/// One task run; optional fields are explicit `null`s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub run_id: String,
    pub task_name: String,
    pub trigger: RunTrigger,
    pub scheduled_for: Option<Ms>,
    pub started_at: Ms,
    pub finished_at: Option<Ms>,
    pub status: RunStatus,
    /// The failure message, or the reason a `degraded` run skipped its work.
    pub error: Option<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    #[serde(rename = "debug")]
    Debug,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warn")]
    Warn,
    #[serde(rename = "error")]
    Error,
}

/// One captured log line (`TaskRunLogLine`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunLogLine {
    /// Epoch ms of the log call.
    pub t: Ms,
    pub level: LogLevel,
    /// Logger name, e.g. `"Main:LiveCheck"`.
    pub logger: String,
    pub msg: String,
}

/// `GET /api/task-runs` body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunsResponse {
    pub runs: Vec<Run>,
}

/// `GET /api/task-runs/:id/logs` body; also the SSE `init` frame payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunLogsResponse {
    pub run: Run,
    pub lines: Vec<RunLogLine>,
    pub dropped: u64,
}

/// Decoded frames of `/api/task-runs/:id/logs/stream` (`ping` frames are ignored).
#[derive(Clone, Debug, PartialEq)]
pub enum RunLogStreamFrame {
    /// `event: init`: everything buffered so far; replaces client state.
    Init(RunLogsResponse),
    /// `event: line`.
    Line(RunLogLine),
    /// `event: done`: the settled run.
    Done(Run),
}

impl RunLogStreamFrame {
    /// The SSE event name.
    pub fn event_name(&self) -> &'static str {
        match self {
            RunLogStreamFrame::Init(_) => "init",
            RunLogStreamFrame::Line(_) => "line",
            RunLogStreamFrame::Done(_) => "done",
        }
    }

    /// Decodes a frame from its SSE event name and data; `None` for unknown events.
    pub fn decode(event: &str, data: &str) -> Option<Result<Self, serde_json::Error>> {
        match event {
            "init" => Some(serde_json::from_str(data).map(RunLogStreamFrame::Init)),
            "line" => Some(serde_json::from_str(data).map(RunLogStreamFrame::Line)),
            "done" => Some(serde_json::from_str(data).map(RunLogStreamFrame::Done)),
            _ => None,
        }
    }
}
