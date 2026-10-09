//! Run trigger and status badges (`components/badges.tsx`).

use leptos::prelude::*;
use omni_api::runs::{RunStatus, RunTrigger};

pub fn trigger_str(trigger: RunTrigger) -> &'static str {
    match trigger {
        RunTrigger::Schedule => "schedule",
        RunTrigger::Manual => "manual",
        RunTrigger::Startup => "startup",
        RunTrigger::Catchup => "catchup",
    }
}

pub fn trigger_label(trigger: RunTrigger) -> &'static str {
    match trigger {
        RunTrigger::Catchup => "catch-up",
        other => trigger_str(other),
    }
}

pub fn status_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Success => "success",
        RunStatus::Error => "error",
    }
}

#[component]
pub fn TriggerBadge(trigger: RunTrigger) -> impl IntoView {
    view! {
        <span class=format!("trigger-badge trigger-{}", trigger_str(trigger))>
            {trigger_label(trigger)}
        </span>
    }
}

#[component]
pub fn StatusDot(status: RunStatus) -> impl IntoView {
    view! { <span class=format!("status-dot status-{}", status_str(status))></span> }
}
