//! `capture-golden`: read-only fixture capture from a running service.
//!
//! Captures hold production data, so they are written only to the gitignored
//! `.local/golden-capture/`; `golden-synthesize` derives the committed fixtures.
//! Only GET requests and the MCP `initialize` + `tools/list` handshake are sent. The
//! bearer token is read from the environment variable named by `--mcp-token-env` and
//! never printed.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{flag_value, flag_values, repo_root};

const USER_AGENT: &str = "OpenAI File Downloader, XaiImageApiFetch/1.0";

/// Read routes without path parameters; routes with ids
/// are passed with `--route /api/workspaces/<id>`.
pub const DEFAULT_ROUTES: &[&str] = &[
    "/api/health",
    "/api/tasks",
    "/api/task-runs",
    "/api/snapshot",
    "/api/streamers",
    "/api/trigger-channels",
    "/api/costs",
    "/api/costs?days=7",
    "/api/costs?days=all",
    "/api/data/entities",
    "/api/email-activity",
    "/api/email-feedback",
    "/api/email-rules",
    "/api/pets",
    "/api/podcast-recommendations",
    "/api/podcast-recommendations/taste-profile",
    "/api/press-pods/episodes",
    "/api/recommendations",
    "/api/recommendations/taste-profile",
    "/api/reminders/status",
    "/api/workspace-papercuts",
    "/api/workspaces",
    "/api/briefings",
    "/pods/rss",
];

/// `/api/costs?days=7` -> `api_costs__days_7`.
pub fn route_slug(route: &str) -> String {
    let mut slug = String::new();
    for c in route.trim_start_matches('/').chars() {
        match c {
            '?' => slug.push_str("__"),
            c if c.is_ascii_alphanumeric() || c == '-' => slug.push(c),
            _ => slug.push('_'),
        }
    }
    slug
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building HTTP client")
}

pub fn capture_golden(args: &[String]) -> Result<()> {
    let base = flag_value(args, "--base").context("--base URL is required")?;
    let base = base.trim_end_matches('/');
    let mut routes: Vec<&str> = DEFAULT_ROUTES.to_vec();
    routes.extend(flag_values(args, "--route"));
    let token = match flag_value(args, "--mcp-token-env") {
        Some(name) => Some(std::env::var(name).with_context(|| format!("${name} is not set"))?),
        None => None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let client = client()?;
        let http_dir = repo_root().join(crate::golden_synth::RAW_DIR);
        std::fs::create_dir_all(&http_dir)?;
        for route in routes {
            let response = client
                .get(format!("{base}{route}"))
                .send()
                .await
                .with_context(|| format!("GET {route}"))?;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let body = response.bytes().await?;
            let slug = route_slug(route);
            if content_type.contains("json") {
                let value: Value = serde_json::from_slice(&body)
                    .with_context(|| format!("{route} returned invalid JSON"))?;
                let fixture = json!({"route": route, "status": status, "body": value});
                write_json(&http_dir.join(format!("{slug}.json")), &fixture)?;
            } else {
                std::fs::write(http_dir.join(format!("{slug}.body")), &body)?;
                write_json(
                    &http_dir.join(format!("{slug}.meta.json")),
                    &json!({"route": route, "status": status, "contentType": content_type}),
                )?;
            }
            println!("captured {route} ({status})");
        }
        if let Some(token) = token {
            let tools = mcp_tools_list(&client, base, &token).await?;
            let live = repo_root().join(".local/golden-capture/tools-list.live.json");
            write_json(&live, &tools)?;
            let offline: Value = serde_json::from_str(omni_mcp_kit::golden::TOOLS_LIST_JSON)?;
            if offline == tools {
                println!("live tools/list matches the offline golden");
            } else {
                println!(
                    "live tools/list DIFFERS from the offline golden; see {}",
                    live.display()
                );
            }
        }
        println!(
            "raw captures are in {}; run `cargo xtask golden-synthesize`",
            http_dir.display()
        );
        Ok(())
    })
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    std::fs::write(
        path,
        format!("{}\n", omni_core::js::json_stringify_pretty2(value)),
    )
    .with_context(|| format!("writing {}", path.display()))
}

/// The first JSON-RPC message of a JSON or SSE response body.
pub fn rpc_message(body: &str) -> Result<Value> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        return Ok(serde_json::from_str(trimmed)?);
    }
    for line in trimmed.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if !data.is_empty() {
                return Ok(serde_json::from_str(data)?);
            }
        }
    }
    bail!("no JSON-RPC message in response")
}

async fn mcp_tools_list(client: &reqwest::Client, base: &str, token: &str) -> Result<Value> {
    const VERSION: &str = "2025-11-25";
    let url = format!("{base}/mcp");
    let post = |body: Value, session: Option<&str>| {
        let mut request = client
            .post(&url)
            .bearer_auth(token)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", VERSION)
            .json(&body);
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request.send()
    };
    let init = post(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": VERSION,
            "capabilities": {},
            "clientInfo": {"name": "omni-xtask-capture", "version": "1.0.0"}
        }}),
        None,
    )
    .await?;
    if !init.status().is_success() {
        bail!("MCP initialize failed with {}", init.status());
    }
    let session = init
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    init.text().await?;
    post(
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        session.as_deref(),
    )
    .await?;
    let list = post(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        session.as_deref(),
    )
    .await?;
    let message = rpc_message(&list.text().await?)?;
    message
        .get("result")
        .cloned()
        .context("tools/list returned no result")
}

/// `git diff --exit-code` over the committed golden directories.
pub fn committed_fixtures_unchanged() -> Result<()> {
    let status = std::process::Command::new("git")
        .args(["diff", "--quiet", "--exit-code", "--"])
        .args([
            ":(glob)crates/*/tests/golden/**",
            ":(glob)crates/*/golden/**",
            "docs/mcp-policy.json",
        ])
        .current_dir(repo_root())
        .status()
        .context("running git diff")?;
    if !status.success() {
        bail!("committed golden fixtures have uncommitted changes");
    }
    println!("golden fixtures: no uncommitted changes");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_file_safe() {
        assert_eq!(route_slug("/api/costs?days=7"), "api_costs__days_7");
        assert_eq!(route_slug("/pods/rss"), "pods_rss");
    }

    #[test]
    fn reads_json_and_sse_rpc_bodies() {
        assert_eq!(rpc_message("{\"id\":1}").unwrap(), json!({"id": 1}));
        assert_eq!(
            rpc_message("event: message\ndata: {\"id\":2}\n\n").unwrap(),
            json!({"id": 2})
        );
    }
}
