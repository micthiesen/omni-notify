//! MCP policy and call status pills.

use leptos::prelude::*;
use omni_api::mcp_activity::{McpCallStatus, RecommendedPolicy};

pub fn policy_str(policy: RecommendedPolicy) -> &'static str {
    match policy {
        RecommendedPolicy::Allow => "allow",
        RecommendedPolicy::RequireApproval => "require_approval",
        RecommendedPolicy::Block => "block",
    }
}

pub fn call_status_str(status: McpCallStatus) -> &'static str {
    match status {
        McpCallStatus::Running => "running",
        McpCallStatus::Ok => "ok",
        McpCallStatus::Error => "error",
        McpCallStatus::Interrupted => "interrupted",
    }
}

fn policy_label(policy: &str) -> String {
    match policy {
        "allow" => "Allow".to_owned(),
        "require_approval" => "Approval".to_owned(),
        "block" => "Block".to_owned(),
        other => other.to_owned(),
    }
}

/// `mcp-policy mcp-policy-<policy>`; unknown policies show their raw value.
#[component]
pub fn PolicyBadge(#[prop(into)] policy: String) -> impl IntoView {
    view! {
        <span
            class=format!("mcp-policy mcp-policy-{policy}")
            title=format!("Recommended policy: {}", policy.replace('_', " "))
        >
            {policy_label(&policy)}
        </span>
    }
}

#[component]
pub fn CallStatusPill(status: McpCallStatus) -> impl IntoView {
    let label = match status {
        McpCallStatus::Running => "Running",
        McpCallStatus::Ok => "OK",
        McpCallStatus::Error => "Error",
        McpCallStatus::Interrupted => "Interrupted",
    };
    view! {
        <span class=format!("mcp-status mcp-status-{}", call_status_str(status))>
            <span class="mcp-status-dot" aria-hidden="true"></span>
            {label}
        </span>
    }
}
