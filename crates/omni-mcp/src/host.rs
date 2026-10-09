//! The Claude Code host seam.
//!
//! Claude session tools, the activity routes and the session watcher reach the
//! host only through this trait. Production wires it to
//! `omni_device_link::DeviceLinkService` (the outbound long-poll relay); the
//! adapter lives in the binary because subsystem crates may not depend on each
//! other.

use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Map, Value};

/// The bounded commands of the host's session client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostCommand {
    Projects,
    List,
    Status,
    Read,
    Result,
    Wait,
    Start,
    Send,
    Stop,
}

impl HostCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            HostCommand::Projects => "projects",
            HostCommand::List => "list",
            HostCommand::Status => "status",
            HostCommand::Read => "read",
            HostCommand::Result => "result",
            HostCommand::Wait => "wait",
            HostCommand::Start => "start",
            HostCommand::Send => "send",
            HostCommand::Stop => "stop",
        }
    }
}

/// `DeviceLinkStatus` of a configured link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLinkStatus {
    pub online: bool,
    pub disabled: bool,
    /// The host's own name; never surfaced through MCP.
    pub host: Option<String>,
    pub last_seen_at: Option<String>,
    pub pending_jobs: usize,
}

/// A relay failure (`DeviceLinkError`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{detail} ({code})")]
pub struct HostError {
    pub code: String,
    pub detail: String,
    pub retryable: bool,
}

impl HostError {
    pub fn new(code: impl Into<String>, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            retryable,
        }
    }
}

/// The host link: status plus bounded command execution.
pub trait ClaudeHost: Send + Sync {
    fn status(&self) -> HostLinkStatus;
    /// Runs one command and returns the client's `data` payload.
    fn execute(
        &self,
        command: HostCommand,
        args: Map<String, Value>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<Map<String, Value>, HostError>>;
}
