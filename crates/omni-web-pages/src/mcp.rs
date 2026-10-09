//! MCP tool-call activity (`pages/McpPage.tsx`): visibility-aware 10 s polling,
//! status/tool filters and "Load Older" paging.

use std::collections::HashSet;
use std::time::Duration;

use leptos::prelude::*;
use omni_api::mcp_activity::{
    McpActivityResponse, McpActivitySummary, McpCall, McpCallStatus, McpToolSummary,
};
use omni_web_kit::api::{self, ApiClientError};
use omni_web_kit::components::mcp_badges::policy_str;
use omni_web_kit::components::{CallStatusPill, PolicyBadge};
use omni_web_kit::hooks::{use_now, use_visible_poll};
use omni_web_kit::router::Link;
use omni_web_kit::task::{TaskHandle, spawn_detached};
use omni_web_kit::utils::claude_activity::{format_json, is_claude_tool};
use omni_web_kit::utils::format::{format_absolute, format_duration, format_relative_at};
use omni_web_kit::utils::js::{js_round, locale_number, number_string};

use crate::common::active_if;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusFilter {
    All,
    Error,
    Running,
}

impl StatusFilter {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Error => "error",
            Self::Running => "running",
        }
    }

    fn status(self) -> Option<McpCallStatus> {
        match self {
            Self::All => None,
            Self::Error => Some(McpCallStatus::Error),
            Self::Running => Some(McpCallStatus::Running),
        }
    }
}

const STATUS_FILTERS: [(StatusFilter, &str); 3] = [
    (StatusFilter::All, "All"),
    (StatusFilter::Error, "Errors"),
    (StatusFilter::Running, "Running"),
];

/// `errorMessage`: the error's message, else the fallback.
pub(crate) fn error_message(message: &str, fallback: &str) -> String {
    if message.is_empty() {
        fallback.to_owned()
    } else {
        message.to_owned()
    }
}

fn stat_tiles(summary: McpActivitySummary) -> impl IntoView {
    let tone = |on: bool, tone: &'static str| if on { tone } else { "" };
    let tiles = [
        ("Stored Calls", summary.stored, ""),
        ("Last 24h", summary.last24h, ""),
        (
            "Errors 24h",
            summary.errors24h,
            tone(summary.errors24h > 0, "danger"),
        ),
        (
            "Running",
            summary.running,
            tone(summary.running > 0, "accent"),
        ),
        (
            "Approval Calls 24h",
            summary.approval_calls24h,
            tone(summary.approval_calls24h > 0, "warn"),
        ),
    ];
    view! {
        <div class="stat-strip">
            {tiles
                .into_iter()
                .map(|(label, value, tone)| {
                    view! {
                        <div class=format!("stat-tile {tone}")>
                            <span class="stat-label">{label}</span>
                            <span class="stat-value">{locale_number(value as f64)}</span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

fn tool_card(
    tool: McpToolSummary,
    active_tool: RwSignal<String>,
    now: ReadSignal<f64>,
) -> impl IntoView {
    let error_rate = if tool.calls > 0 {
        tool.errors as f64 / tool.calls as f64
    } else {
        0.0
    };
    let name = tool.tool.clone();
    let is_active = {
        let name = name.clone();
        move || active_tool.with(|t| *t == name)
    };
    let class_active = is_active.clone();
    let pressed = is_active.clone();
    let last_at = tool.last_at as f64;
    view! {
        <button
            type="button"
            class=move || format!("mcp-tool-card {}", active_if(class_active()))
            aria-pressed=move || pressed().to_string()
            on:click=move |_| active_tool.set(if is_active() { String::new() } else { name.clone() })
        >
            <span class="mcp-tool-card-head">
                <span class="mcp-tool-title">{tool.title.clone()}</span>
                <PolicyBadge policy=policy_str(tool.recommended_policy) />
            </span>
            <code class="mcp-tool-name">{tool.tool.clone()}</code>
            <span class="mcp-tool-stats">
                <span>
                    <strong>{locale_number(tool.calls as f64)}</strong>
                    " calls"
                </span>
                <span class=(tool.errors > 0).then_some("mcp-tool-errors")>
                    <strong>{locale_number(tool.errors as f64)}</strong>
                    " errors"
                    {(tool.errors > 0)
                        .then(|| format!(" ({}%)", number_string(js_round(error_rate * 100.0))))}
                </span>
                {tool
                    .avg_duration_ms
                    .map(|avg| view! { <span>{format!("avg {}", format_duration(avg as f64))}</span> })}
            </span>
            <span class="mcp-tool-last" title=format_absolute(last_at)>
                {move || format!("Last used {}", format_relative_at(last_at, now.get()))}
            </span>
        </button>
    }
}

fn status_str(status: McpCallStatus) -> &'static str {
    match status {
        McpCallStatus::Running => "running",
        McpCallStatus::Ok => "ok",
        McpCallStatus::Error => "error",
        McpCallStatus::Interrupted => "interrupted",
    }
}

fn call_details(call: &McpCall) -> impl IntoView + use<> {
    let claude = is_claude_tool(&call.tool);
    let has_output = !call.output.is_null();
    view! {
        <div class="mcp-call-details">
            <div class="mcp-call-facts meta-row muted">
                <span>{format_absolute(call.started_at as f64)}</span>
                {call
                    .finished_at
                    .map(|f| view! { <span>{format!("finished {}", format_absolute(f as f64))}</span> })}
                <span>{if call.read_only { "read-only" } else { "writes" }}</span>
                <span class="mcp-call-id">{call.call_id.clone()}</span>
            </div>
            {call
                .error
                .clone()
                .filter(|e| !e.is_empty())
                .map(|e| {
                    view! {
                        <div class="mcp-detail-block">
                            <div class="mcp-detail-label">"Error"</div>
                            <pre class="mcp-json mcp-json-error">{e}</pre>
                        </div>
                    }
                })}
            <div class="mcp-detail-block">
                <div class="mcp-detail-label">"Input"</div>
                <pre class="mcp-json">{format_json(&call.input)}</pre>
            </div>
            {has_output
                .then(|| {
                    let output = format_json(&call.output);
                    view! {
                        <div class="mcp-detail-block">
                            <div class="mcp-detail-label">"Output"</div>
                            <pre class="mcp-json">{output}</pre>
                        </div>
                    }
                })}
            {claude.then(|| view! { <Link to="/claude" class="section-view-all">"View in Claude Code ›"</Link> })}
        </div>
    }
}

/// One call; its open state lives in `open_calls` so it survives polls.
fn call_row(
    call: McpCall,
    open_calls: RwSignal<HashSet<String>>,
    now: ReadSignal<f64>,
) -> impl IntoView {
    let id = call.call_id.clone();
    let open = {
        let id = id.clone();
        Memo::new(move |_| open_calls.with(|set| set.contains(&id)))
    };
    let status = status_str(call.status);
    let started = call.started_at as f64;
    let duration_ms = call.duration_ms;
    let running = call.status == McpCallStatus::Running;
    let duration = move || match duration_ms {
        Some(ms) => format_duration(ms as f64),
        None if running => format_duration(now.get() - started),
        None => "—".to_owned(),
    };
    let error_line = call.error.clone().filter(|e| !e.is_empty());
    let detail_call = call.clone();
    view! {
        <li class=move || format!("mcp-call mcp-call-{status} {}", if open.get() { "open" } else { "" })>
            <div class="mcp-call-head">
                <button
                    type="button"
                    class="mcp-call-toggle"
                    aria-expanded=move || open.get().to_string()
                    on:click=move |_| {
                        open_calls
                            .update(|set| {
                                if !set.remove(&id) {
                                    set.insert(id.clone());
                                }
                            })
                    }
                >
                    <span class="mcp-caret" aria-hidden="true">
                        {move || if open.get() { "▾" } else { "▸" }}
                    </span>
                    <span class="mcp-call-main">
                        <span class="mcp-call-title">{call.title.clone()}</span>
                        <code class="mcp-tool-name">{call.tool.clone()}</code>
                    </span>
                    <span class="mcp-call-meta">
                        <CallStatusPill status=call.status />
                        <PolicyBadge policy=policy_str(call.recommended_policy) />
                        <span class="mcp-call-duration">{duration}</span>
                        <span class="mcp-call-time" title=format_absolute(started)>
                            {move || format_relative_at(started, now.get())}
                        </span>
                    </span>
                </button>
            </div>
            {move || {
                error_line
                    .clone()
                    .filter(|_| !open.get())
                    .map(|e| view! { <div class="mcp-call-error-line">{e}</div> })
            }}
            {move || open.get().then(|| call_details(&detail_call))}
        </li>
    }
}

/// Merges the polled first page with older pages, dropping duplicates.
pub(crate) fn merge_calls(head: &[McpCall], older: &[McpCall]) -> Vec<McpCall> {
    let seen: HashSet<&str> = head.iter().map(|c| c.call_id.as_str()).collect();
    head.iter()
        .chain(older.iter().filter(|c| !seen.contains(c.call_id.as_str())))
        .cloned()
        .collect()
}

#[component]
pub fn McpPage() -> impl IntoView {
    let now = use_now(15_000);
    let status_filter = RwSignal::new(StatusFilter::All);
    let tool = RwSignal::new(String::new());
    let head = RwSignal::new(None::<McpActivityResponse>);
    let error = RwSignal::new(None::<String>);
    let older = RwSignal::new((Vec::<McpCall>::new(), None::<i64>));
    let head_key = RwSignal::new(String::new());
    let loading_older = RwSignal::new(false);
    let older_error = RwSignal::new(None::<String>);
    let open_calls = RwSignal::new(HashSet::<String>::new());
    let cancel_older = StoredValue::new(None::<TaskHandle>);

    let filter_key = Memo::new(move |_| format!("{}:{}", status_filter.get().as_str(), tool.get()));
    let filter_status = move || status_filter.get_untracked().status();
    let filter_tool = move || Some(tool.get_untracked()).filter(|t| !t.is_empty());

    use_visible_poll(
        filter_key.into(),
        move || {
            let tool = filter_tool();
            let status = filter_status();
            let key = filter_key.get_untracked();
            async move {
                api::fetch_mcp_activity(Some(100), tool.as_deref(), status, None)
                    .await
                    .map(|response| (key, response))
            }
        },
        move |(key, response): (String, McpActivityResponse)| {
            head.set(Some(response));
            head_key.set(key);
            error.set(None);
        },
        move |err: ApiClientError| {
            error.set(Some(error_message(
                err.message(),
                "Failed to load MCP activity",
            )));
        },
        Duration::from_secs(10),
    );

    // A filter change discards older pages loaded under the previous filter
    // and abandons an in-flight "Load older" request.
    Effect::new(move |_| {
        filter_key.track();
        older.set((Vec::new(), None));
        older_error.set(None);
        loading_older.set(false);
        if let Some(Some(handle)) = cancel_older.try_get_value() {
            handle.abort();
        }
    });

    // Stats and tool summaries stay visible while a new filter loads its calls.
    let calls_current =
        Memo::new(move |_| head.with(Option::is_some) && head_key.with(|k| *k == filter_key.get()));
    let calls = Memo::new(move |_| {
        if !calls_current.get() {
            return Vec::new();
        }
        head.with(|h| {
            h.as_ref()
                .map(|h| older.with(|(o, _)| merge_calls(&h.calls, o)))
                .unwrap_or_default()
        })
    });
    let next_before = Memo::new(move |_| {
        if !calls_current.get() {
            return None;
        }
        older.with(|(calls, next)| {
            if calls.is_empty() {
                head.with(|h| h.as_ref().and_then(|h| h.next_before))
            } else {
                *next
            }
        })
    });

    let load_older = move |_| {
        let Some(before) = next_before.get_untracked() else {
            return;
        };
        if loading_older.get_untracked() {
            return;
        }
        loading_older.set(true);
        older_error.set(None);
        let tool = filter_tool();
        let status = filter_status();
        let handle = spawn_detached(async move {
            match api::fetch_mcp_activity(Some(100), tool.as_deref(), status, Some(before)).await {
                Ok(response) => {
                    older.update(|(calls, next)| {
                        calls.extend(response.calls);
                        *next = response.next_before;
                    });
                    loading_older.set(false);
                }
                Err(err) => {
                    older_error.set(Some(error_message(
                        err.message(),
                        "Failed to load older calls",
                    )));
                    loading_older.set(false);
                }
            }
        });
        cancel_older.set_value(Some(handle));
    };
    on_cleanup(move || {
        if let Some(Some(handle)) = cancel_older.try_get_value() {
            handle.abort();
        }
    });

    let tool_select = move || {
        let current = tool.get();
        let options = head.with(|h| h.as_ref().map(|h| h.tools.clone()).unwrap_or_default());
        let extra =
            (!current.is_empty() && !options.iter().any(|o| o.tool == current)).then(|| {
                let value = current.clone();
                view! { <option value=value.clone() prop:selected=true>{value.clone()}</option> }
            });
        view! {
            <select
                class="task-search mcp-tool-select"
                aria-label="Filter by tool"
                on:change=move |event| tool.set(event_target_value(&event))
            >
                <option value="" prop:selected=current.is_empty()>"All tools"</option>
                {extra}
                {options
                    .into_iter()
                    .map(|option| {
                        let selected = option.tool == current;
                        view! {
                            <option value=option.tool.clone() prop:selected=selected>
                                {format!("{} ({})", option.title, option.tool)}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>
        }
    };

    let summary = Memo::new(move |_| head.with(|h| h.as_ref().map(|h| h.summary)));
    let tools =
        Memo::new(move |_| head.with(|h| h.as_ref().map(|h| h.tools.clone()).unwrap_or_default()));
    let max_calls =
        Memo::new(move |_| head.with(|h| h.as_ref().map_or(0, |h| h.retention.max_calls)));
    let body = move || {
        view! {
            {move || {
                error
                    .get()
                    .map(|e| {
                        view! {
                            <div class="error-inline stale-note">
                                {format!("Refresh failed ({e}), showing last known state.")}
                            </div>
                        }
                    })
            }}
            {move || summary.get().map(stat_tiles)}

            <section class="page-section">
                <div class="section-heading-row">
                    <h2 class="section-title">
                        "Tools " <span class="section-count">{move || tools.with(Vec::len)}</span>
                    </h2>
                </div>
                {move || {
                    let tools = tools.get();
                    if tools.is_empty() {
                        view! { <div class="muted">"No tools called yet."</div> }.into_any()
                    } else {
                        view! {
                            <div class="mcp-tool-grid">
                                {tools.into_iter().map(|t| tool_card(t, tool, now)).collect_view()}
                            </div>
                        }
                            .into_any()
                    }
                }}
            </section>

            <section class="page-section">
                <div class="section-heading-row">
                    <h2 class="section-title">"Calls"</h2>
                    <span class="muted mcp-retention">
                        {move || format!("Keeps the latest {}", locale_number(max_calls.get() as f64))}
                    </span>
                </div>
                <div class="task-toolbar">
                    <div class="task-filters" role="group" aria-label="Call status">
                        {STATUS_FILTERS
                            .iter()
                            .map(|(value, label)| {
                                let value = *value;
                                view! {
                                    <button
                                        type="button"
                                        class=move || format!("chip-btn {}", active_if(status_filter.get() == value))
                                        aria-pressed=move || (status_filter.get() == value).to_string()
                                        on:click=move |_| status_filter.set(value)
                                    >
                                        {*label}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                    {tool_select}
                </div>
                {move || {
                    if !calls_current.get() {
                        view! { <div class="loading-inline">"Loading calls…"</div> }.into_any()
                    } else if calls.with(Vec::is_empty) {
                        view! { <div class="muted">"No calls match this view."</div> }.into_any()
                    } else {
                        view! {
                            <ul class="mcp-call-list">
                                <For
                                    each=move || calls.get()
                                    key=|call| format!("{}|{call:?}", call.call_id)
                                    children=move |call| call_row(call, open_calls, now)
                                />
                            </ul>
                        }
                            .into_any()
                    }
                }}
                {move || older_error.get().map(|e| view! { <div class="error-inline">{e}</div> })}
                {move || {
                    next_before
                        .get()
                        .is_some()
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="show-more-btn"
                                    on:click=load_older
                                    disabled=move || loading_older.get()
                                >
                                    {move || if loading_older.get() { "Loading…" } else { "Load Older" }}
                                </button>
                            }
                        })
                }}
            </section>
        }
    };

    let has_head = Memo::new(move |_| head.with(Option::is_some));
    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"MCP Activity"</h1>
                <p class="page-subtitle">
                    "Every tool call agents made through Omni's MCP server. "
                    <Link to="/claude" class="mcp-inline-link">"Claude Code actions ›"</Link>
                </p>
            </div>
        </div>

        {move || {
            (!has_head.get() && error.with(Option::is_none))
                .then(|| view! { <div class="loading">"Loading…"</div> })
        }}
        {move || {
            error
                .get()
                .filter(|_| !has_head.get())
                .map(|e| {
                    view! {
                        <div class="error">
                            <div>"Failed to load MCP activity"</div>
                            <div class="error-detail">{e}</div>
                        </div>
                    }
                })
        }}
        {move || has_head.get().then(body)}
    }
}

#[cfg(test)]
mod tests {
    use omni_api::mcp_activity::{McpCall, McpCallStatus, RecommendedPolicy};
    use serde_json::Value;

    use super::{error_message, merge_calls};

    fn call(id: &str) -> McpCall {
        McpCall {
            call_id: id.into(),
            tool: "t".into(),
            title: "T".into(),
            recommended_policy: RecommendedPolicy::Allow,
            read_only: true,
            started_at: 1,
            finished_at: None,
            duration_ms: None,
            status: McpCallStatus::Ok,
            error: None,
            input: Value::Null,
            output: Value::Null,
        }
    }

    #[test]
    fn older_pages_append_without_duplicates() {
        let merged = merge_calls(&[call("a"), call("b")], &[call("b"), call("c")]);
        let ids: Vec<&str> = merged.iter().map(|c| c.call_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn empty_messages_fall_back() {
        assert_eq!(error_message("", "Failed"), "Failed");
        assert_eq!(error_message("boom", "Failed"), "boom");
    }
}
