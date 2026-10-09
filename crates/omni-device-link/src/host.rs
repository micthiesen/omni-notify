//! The `omni_runtime::ports::ClaudeHost` port over the relay: the only way MCP
//! tools, activity routes and the session watcher (WP12) reach the host.

use std::time::Duration;

use futures::future::BoxFuture;
use omni_runtime::ports::{ClaudeHost, HostCommand, HostError, HostLinkStatus};
use serde_json::{Map, Value};

use crate::service::{DeviceCommand, DeviceLinkService};

fn device_command(command: HostCommand) -> DeviceCommand {
    match command {
        HostCommand::Projects => DeviceCommand::Projects,
        HostCommand::List => DeviceCommand::List,
        HostCommand::Status => DeviceCommand::Status,
        HostCommand::Read => DeviceCommand::Read,
        HostCommand::Result => DeviceCommand::Result,
        HostCommand::Wait => DeviceCommand::Wait,
        HostCommand::Start => DeviceCommand::Start,
        HostCommand::Send => DeviceCommand::Send,
        HostCommand::Stop => DeviceCommand::Stop,
    }
}

impl ClaudeHost for DeviceLinkService {
    fn status(&self) -> HostLinkStatus {
        let status = DeviceLinkService::status(self);
        HostLinkStatus {
            online: status.online,
            disabled: status.disabled,
            host: status.host,
            last_seen_at: status.last_seen_at,
            pending_jobs: status.pending_jobs,
        }
    }

    fn execute(
        &self,
        command: HostCommand,
        args: Map<String, Value>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<Map<String, Value>, HostError>> {
        Box::pin(async move {
            DeviceLinkService::execute(self, device_command(command), args, timeout)
                .await
                .map_err(|e| HostError::new(e.code, e.detail, e.retryable))
        })
    }
}
