//! End to end: Claude session tools over the real device-link relay.
//!
//! `DeviceLinkAdapter` is the binary's adapter (subsystem crates may not
//! depend on each other, so WP14 owns the production copy of these ~30
//! lines). A simulated host long-polls the relay and answers with canned
//! session-client envelopes; results must reach MCP with the host name and
//! home directory scrubbed, and errors keep their codes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use futures::future::BoxFuture;
use omni_device_link::{DeviceCommand, DeviceJobOutcome, DeviceLinkService, PollReport};
use omni_mcp::host::{ClaudeHost, HostCommand, HostError, HostLinkStatus};
use omni_mcp::tools::claude_sessions::{ClaudeDeps, claude_session_tools};
use serde_json::{Map, Value, json};

struct DeviceLinkAdapter(DeviceLinkService);

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

impl ClaudeHost for DeviceLinkAdapter {
    fn status(&self) -> HostLinkStatus {
        let status = self.0.status();
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
            self.0
                .execute(device_command(command), args, timeout)
                .await
                .map_err(|e| HostError::new(e.code, e.detail, e.retryable))
        })
    }
}

/// Answers every job the way the host's session client would.
fn spawn_host(link: DeviceLinkService) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let jobs = link
                .poll(PollReport {
                    disabled: false,
                    host: Some("MaxBook".to_owned()),
                })
                .await;
            for job in jobs {
                let outcome = match job.command {
                    DeviceCommand::Projects => DeviceJobOutcome::Output(json!({
                        "v": 1, "ok": true,
                        "data": {"projects": [{"name": "omni-notify", "path": "/Users/michael/Code/omni-notify", "exists": true}]}
                    })),
                    DeviceCommand::List => DeviceJobOutcome::Output(json!({
                        "v": 1, "ok": true,
                        "data": {"sessions": [{
                            "session_id": "abc-123", "status": "idle", "revision": 2,
                            "cwd": "/Users/michael/Code/omni-notify",
                            "last_assistant": "Built it on MaxBook."
                        }]}
                    })),
                    _ => DeviceJobOutcome::Output(json!({
                        "v": 1, "ok": false,
                        "error": {"code": "busy", "message": "MaxBook session /Users/michael/x is mid-turn"}
                    })),
                };
                link.complete(&job.id, outcome);
            }
        }
    })
}

#[tokio::test]
async fn claude_tools_reach_the_host_only_through_the_long_poll_and_never_name_it() {
    let clock = clock();
    let db = test_store(&clock).await;
    let link = DeviceLinkService::new(clock.clone());
    let host = spawn_host(link.clone());
    let tools = claude_session_tools(&ClaudeDeps {
        host: Some(Arc::new(DeviceLinkAdapter(link.clone()))),
        watcher: None,
    })
    .unwrap();
    let router = mcp_router(&db.store, &clock, tools, None);
    // Let the simulated host's first poll register.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let status = call_tool(&router, "claude_link_status", json!({})).await;
    let body = &status["result"]["structuredContent"];
    assert_eq!(body["online"], true);
    assert_eq!(body["projects"][0]["path"], "~/Code/omni-notify");
    assert!(body.get("host").is_none());

    let listed = call_tool(&router, "claude_sessions_list", json!({})).await;
    let session = &listed["result"]["structuredContent"]["sessions"][0];
    assert_eq!(session["cwd"], "~/Code/omni-notify");
    assert_eq!(session["lastAssistant"], "Built it on the host.");

    let failed = call_tool(
        &router,
        "claude_session_send",
        json!({"session": "abc-123", "prompt": "go"}),
    )
    .await;
    assert!(is_error(&failed));
    assert_eq!(
        error_text(&failed),
        "the host session ~/x is mid-turn (busy)"
    );
    let text = format!("{status}{listed}{failed}");
    assert!(!text.contains("MaxBook"));
    assert!(!text.contains("/Users/"));
    host.abort();
}
