//! Claude Code host link, live sessions, transcripts and the `claude_*`
//! action timeline (`pages/ClaudePage.tsx`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use leptos::html::Div;
use leptos::prelude::*;
use omni_api::claude::{
    ClaudeActivityResponse, ClaudeLinkView, ClaudeSession, ClaudeTranscriptItem,
    ClaudeTranscriptResponse,
};
use omni_api::mcp_activity::{McpCall, McpCallStatus};
use omni_web_kit::api::{self, ApiClientError, ClaudeLinkError, ClaudeLinkGetError};
use omni_web_kit::components::CallStatusPill;
use omni_web_kit::hooks::{use_modal, use_now, use_visible_poll};
use omni_web_kit::markdown::WorkspaceMarkdown;
use omni_web_kit::router::Link;
use omni_web_kit::task::spawn_scoped;
use omni_web_kit::utils::claude_activity::{
    ClaudeActionGroup, ClaudeActionKind, boolean_field, claude_action_kind, explain_claude_error,
    group_claude_actions, number_field, output_session, parse_iso_ms, short_session_id, snippet,
    string_field, summarize_compact_action, tool_input_summary,
};
use omni_web_kit::utils::format::{format_absolute, format_duration, format_relative_at};
use omni_web_kit::utils::js::{date_locale_time_string, locale_compare, number_string, utf16_len};

use crate::common::active_if;
use crate::mcp::error_message;

const TIMELINE_PAGE: usize = 6;
const POLL: Duration = Duration::from_secs(10);

/// Opens the transcript modal for `(session_id, title)`.
type OpenTranscript = Callback<(String, String)>;

// ===== Link status =====

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkState {
    Online,
    Offline,
    Disabled,
    Unconfigured,
}

impl LinkState {
    fn of(link: &ClaudeLinkView) -> Self {
        if !link.configured {
            Self::Unconfigured
        } else if link.disabled {
            Self::Disabled
        } else if link.online {
            Self::Online
        } else {
            Self::Offline
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Disabled => "disabled",
            Self::Unconfigured => "unconfigured",
        }
    }

    /// `LINK_COPY`: `(label, detail)`.
    fn copy(self) -> (&'static str, &'static str) {
        match self {
            Self::Online => ("Online", "The Mac is polling Omni for Claude Code jobs."),
            Self::Offline => (
                "Offline",
                "The Mac has stopped polling. It may be asleep, away from home without VPN, or the omni-link agent stopped. Jobs sent now will not be picked up.",
            ),
            Self::Disabled => (
                "Disabled",
                "The kill switch is on, so Claude tools refuse to act. Run `omni-link enable` on the Mac to turn it back on.",
            ),
            Self::Unconfigured => (
                "Not Configured",
                "The Mac device link is not configured on this server.",
            ),
        }
    }
}

fn link_hero(link: ClaudeLinkView, now: ReadSignal<f64>) -> impl IntoView {
    let state = LinkState::of(&link);
    let (label, detail) = state.copy();
    let last_seen = parse_iso_ms(link.last_seen_at.as_deref());
    let pending = link.pending_jobs;
    view! {
        <section class=format!("claude-hero claude-hero-{}", state.as_str()) aria-label="Mac link status">
            <div class="claude-hero-status">
                <span class="claude-hero-dot" aria-hidden="true"></span>
                <div class="claude-hero-text">
                    <div class="claude-hero-label">"Mac Link · " <strong>{label}</strong></div>
                    <p class="claude-hero-detail">{detail}</p>
                </div>
            </div>
            <dl class="claude-hero-facts">
                <div>
                    <dt>"Host"</dt>
                    <dd class="claude-mono">{link.host.clone().unwrap_or_else(|| "—".to_owned())}</dd>
                </div>
                <div>
                    <dt>"Last Seen"</dt>
                    <dd title=last_seen.map(format_absolute)>
                        {move || match last_seen {
                            None => "Never".to_owned(),
                            Some(at) => format_relative_at(at, now.get()),
                        }}
                    </dd>
                </div>
                <div>
                    <dt>"Pending Jobs"</dt>
                    <dd class=(pending > 0).then_some("claude-hero-pending")>{pending}</dd>
                </div>
            </dl>
        </section>
    }
}

fn link_error_notice(error: &ClaudeLinkError) -> impl IntoView + use<> {
    let hint = explain_claude_error(Some(&error.code)).hint;
    view! {
        <div class=format!("claude-link-notice claude-link-notice-{}", error.code)>
            <strong>{error.message.clone()}</strong>
            {hint.map(|h| view! { <span>{h}</span> })}
        </div>
    }
}

// ===== Shared bits =====

fn session_status(status: Option<String>) -> impl IntoView {
    status.filter(|s| !s.is_empty()).map(|status| {
        let class = format!("claude-status claude-status-{status}");
        view! {
            <span class=class>
                <span class="claude-status-dot" aria-hidden="true"></span>
                {status}
            </span>
        }
    })
}

/// Long prompts clamp to six lines with a toggle; `open` survives re-renders.
fn expandable_text(text: String, class: &'static str, open: RwSignal<bool>) -> impl IntoView {
    let lines = 6;
    let long = utf16_len(&text) > lines * 90 || text.split('\n').count() > lines;
    view! {
        <div class="claude-expandable">
            <div
                class=move || format!("{class} {}", if long && !open.get() { "claude-clamped" } else { "" })
                style=move || (long && !open.get()).then(|| format!("-webkit-line-clamp: {lines};"))
            >
                {text}
            </div>
            {long
                .then(|| {
                    view! {
                        <button type="button" class="claude-more-btn" on:click=move |_| open.update(|o| *o = !*o)>
                            {move || if open.get() { "Show less" } else { "Show more" }}
                        </button>
                    }
                })}
        </div>
    }
}

fn tag(text: impl Into<String>, tone: Option<&'static str>) -> impl IntoView {
    let class = format!(
        "claude-tag {}",
        tone.map(|t| format!("claude-tag-{t}")).unwrap_or_default()
    );
    view! { <span class=class>{text.into()}</span> }
}

// ===== Transcript =====

fn transcript_item(item: ClaudeTranscriptItem) -> impl IntoView {
    let time = parse_iso_ms(item.timestamp.as_deref());
    let stamp = move || {
        time.map(|time| {
            view! {
                <time class="claude-tx-time" title=format_absolute(time)>
                    {date_locale_time_string(time, &[("hour", "numeric"), ("minute", "2-digit")])}
                </time>
            }
        })
    };
    let truncated = move || {
        item.truncated
            .then(|| view! { <span class="claude-tag">"truncated"</span> })
    };
    match item.kind.as_str() {
        "user" => view! {
            <div class="claude-tx-row claude-tx-user">
                <div class="claude-bubble claude-bubble-user">
                    {item.text.clone().unwrap_or_default()}
                    <div class="claude-tx-foot">{truncated()} {stamp()}</div>
                </div>
            </div>
        }
        .into_any(),
        "assistant" => view! {
            <div class="claude-tx-row claude-tx-assistant">
                <div class="claude-bubble claude-bubble-assistant">
                    <WorkspaceMarkdown content=item.text.clone().unwrap_or_default() />
                    <div class="claude-tx-foot">{truncated()} {stamp()}</div>
                </div>
            </div>
        }
        .into_any(),
        "tool_use" => {
            let input = item.input.clone().filter(|i| !i.is_empty());
            let summary = input.as_ref().map(|i| {
                view! { <span class="claude-tool-input">{tool_input_summary(Some(i))}</span> }
            });
            view! {
                <div class="claude-tx-row">
                    <details class="claude-tool-chip">
                        <summary>
                            <span class="claude-tool-name">{item.tool.clone().unwrap_or_else(|| "tool".to_owned())}</span>
                            {summary}
                        </summary>
                        {input.map(|i| view! { <pre class="mcp-json">{i}</pre> })}
                    </details>
                </div>
            }
            .into_any()
        }
        "tool_result" => {
            let is_error = item.is_error.unwrap_or(false);
            let short = snippet(item.text.as_deref(), 100);
            view! {
                <div class="claude-tx-row">
                    <details class=format!(
                        "claude-tool-result {}",
                        if is_error { "claude-tool-result-error" } else { "" },
                    )>
                        <summary>
                            {if is_error { "Tool error" } else { "Tool result" }}
                            {item
                                .tool
                                .clone()
                                .filter(|t| !t.is_empty())
                                .map(|t| view! { <span class="claude-tool-name">{t}</span> })}
                            {short.map(|s| view! { <span class="claude-tool-input">{s}</span> })}
                            {truncated()}
                        </summary>
                        <pre class="mcp-json">{item.text.clone().unwrap_or_else(|| "(empty)".to_owned())}</pre>
                    </details>
                </div>
            }
            .into_any()
        }
        _ => view! {
            <div class="claude-tx-row">
                <div class="claude-tx-other muted">
                    <span class="claude-tag">{item.kind.clone()}</span>
                    " "
                    {item.text.clone().unwrap_or_default()}
                </div>
            </div>
        }
        .into_any(),
    }
}

#[component]
fn TranscriptModal(session_id: String, title: String, on_close: Callback<()>) -> impl IntoView {
    let transcript = RwSignal::new(None::<ClaudeTranscriptResponse>);
    let error = RwSignal::new(None::<ClaudeLinkGetError>);
    let loading = RwSignal::new(true);
    let version = RwSignal::new(0u32);
    let modal_ref = use_modal(move || on_close.run(()));
    let body_ref = NodeRef::<Div>::new();

    let fetch_id = session_id.clone();
    Effect::new(move |_| {
        version.track();
        loading.set(true);
        let id = fetch_id.clone();
        spawn_scoped(async move {
            match api::fetch_claude_transcript(&id, 40, None).await {
                Ok(next) => {
                    transcript.set(Some(next));
                    error.set(None);
                    loading.set(false);
                }
                Err(err) => {
                    error.set(Some(err));
                    loading.set(false);
                }
            }
        });
    });

    Effect::new(move |_| {
        if transcript.with(Option::is_some) {
            request_animation_frame(move || {
                if let Some(body) = body_ref.get_untracked() {
                    body.set_scroll_top(body.scroll_height());
                }
            });
        }
    });

    view! {
        <div class="modal-root">
            <button
                type="button"
                class="modal-backdrop"
                tabindex="-1"
                on:click=move |_| on_close.run(())
                aria-label="Close transcript"
            ></button>
            <div
                class="log-modal claude-tx-modal"
                node_ref=modal_ref
                tabindex="-1"
                aria-modal="true"
                role="dialog"
                aria-label=format!("Transcript for {title}")
            >
                <div class="log-modal-header">
                    <div class="log-modal-title">
                        <span class="log-modal-task">{title.clone()}</span>
                    </div>
                    <div class="log-modal-meta meta-row muted">
                        <span class="claude-mono">{short_session_id(&session_id)}</span>
                        {move || transcript.with(|t| t.as_ref().map(|t| view! { <span>{format!("rev {}", t.revision)}</span> }))}
                        <button
                            type="button"
                            class="claude-link-btn"
                            on:click=move |_| version.update(|v| *v += 1)
                            disabled=move || loading.get()
                        >
                            {move || if loading.get() { "Loading…" } else { "Refresh" }}
                        </button>
                    </div>
                    <button type="button" class="log-modal-close" on:click=move |_| on_close.run(()) aria-label="Close">
                        "✕"
                    </button>
                </div>
                <div class="claude-tx-body" node_ref=body_ref>
                    {move || {
                        transcript
                            .with(|t| t.as_ref().filter(|t| t.has_more).map(|t| t.items.len()))
                            .map(|count| {
                                view! {
                                    <div class="claude-tx-note muted">
                                        {format!(
                                            "Showing the latest {count} items. Earlier history stays on the Mac.",
                                        )}
                                    </div>
                                }
                            })
                    }}
                    {move || {
                        error
                            .get()
                            .map(|error| match error {
                                ClaudeLinkGetError::Link(link) => link_error_notice(&link).into_any(),
                                ClaudeLinkGetError::Client(client) => {
                                    view! {
                                        <div class="error-inline">
                                            {error_message(client.message(), "Failed to load transcript")}
                                        </div>
                                    }
                                        .into_any()
                                }
                            })
                    }}
                    {move || {
                        (transcript.with(Option::is_none) && error.with(Option::is_none))
                            .then(|| view! { <div class="loading-inline">"Loading…"</div> })
                    }}
                    {move || {
                        transcript
                            .with(|t| t.as_ref().is_some_and(|t| t.items.is_empty()))
                            .then(|| view! { <div class="muted claude-tx-note">"No transcript items yet."</div> })
                    }}
                    {move || {
                        transcript
                            .get()
                            .map(|t| t.items.into_iter().map(transcript_item).collect_view())
                    }}
                </div>
            </div>
        </div>
    }
}

// ===== Live sessions =====

fn session_card(
    session: ClaudeSession,
    now: ReadSignal<f64>,
    on_open: OpenTranscript,
) -> impl IntoView {
    let started = parse_iso_ms(session.started_at.as_deref());
    let title = session
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled session".to_owned());
    let open_id = session.session_id.clone();
    let open_title = title.clone();
    view! {
        <article class=format!("claude-session-card claude-session-{}", session.status)>
            <header class="claude-session-head">
                <h3 class="claude-session-title">{title}</h3>
                {session_status(Some(session.status.clone()))}
            </header>
            <div class="claude-session-meta meta-row muted">
                {session
                    .project
                    .clone()
                    .filter(|p| !p.is_empty())
                    .map(|p| view! { <span class="claude-project">{p}</span> })}
                {session.kind.clone().filter(|k| !k.is_empty()).map(|k| view! { <span>{k}</span> })}
                {started
                    .map(|at| {
                        view! {
                            <span title=format_absolute(at)>
                                {move || format!("started {}", format_relative_at(at, now.get()))}
                            </span>
                        }
                    })}
                <span>{format!("rev {}", session.revision)}</span>
                <span class="claude-mono">
                    {session.id.clone().unwrap_or_else(|| short_session_id(&session.session_id))}
                </span>
            </div>
            {match session.last_assistant.clone().filter(|a| !a.is_empty()) {
                Some(text) => view! { <p class="claude-session-excerpt">{text}</p> }.into_any(),
                None => view! { <p class="claude-session-excerpt muted">"No assistant reply yet."</p> }.into_any(),
            }}
            <div class="claude-session-actions">
                <button
                    type="button"
                    class="run-btn"
                    on:click=move |_| on_open.run((open_id.clone(), open_title.clone()))
                >
                    "Transcript"
                </button>
            </div>
        </article>
    }
}

/// `[(name, count)]` sorted by `localeCompare`.
fn sorted_counts(counts: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut entries: Vec<(String, usize)> = counts.into_iter().collect();
    entries.sort_by(|a, b| locale_compare(&a.0, &b.0));
    entries
}

fn project_chips(projects: Vec<(String, usize)>, project: RwSignal<String>) -> impl IntoView {
    projects
        .into_iter()
        .map(|(name, count)| {
            let is_active = {
                let name = name.clone();
                move || project.with(|p| *p == name)
            };
            let class_active = is_active.clone();
            let pressed = is_active.clone();
            let label = name.clone();
            view! {
                <button
                    type="button"
                    class=move || format!("chip-btn {}", active_if(class_active()))
                    aria-pressed=move || pressed().to_string()
                    on:click=move |_| project.set(if is_active() { String::new() } else { name.clone() })
                >
                    {label}
                    " "
                    <span class="chip-btn-count">{count}</span>
                </button>
            }
        })
        .collect_view()
}

#[component]
fn SessionsSection(now: ReadSignal<f64>, on_open_transcript: OpenTranscript) -> impl IntoView {
    let include_stopped = RwSignal::new(false);
    let sessions = RwSignal::new(None::<Vec<ClaudeSession>>);
    let error = RwSignal::new(None::<ClaudeLinkGetError>);
    let project = RwSignal::new(String::new());

    use_visible_poll(
        Signal::derive(move || format!("sessions:{}", include_stopped.get())),
        move || {
            let include = include_stopped.get_untracked();
            async move { api::fetch_claude_sessions(include, 25).await }
        },
        move |response: omni_api::claude::ClaudeSessionsResponse| {
            sessions.set(Some(response.sessions));
            error.set(None);
        },
        move |err: ClaudeLinkGetError| {
            // The host is unreachable; a stale list would suggest it is still live.
            if matches!(err, ClaudeLinkGetError::Link(_)) {
                sessions.set(None);
            }
            error.set(Some(err));
        },
        POLL,
    );

    let projects = Memo::new(move |_| {
        let mut counts = HashMap::<String, usize>::new();
        sessions.with(|s| {
            for session in s.iter().flatten() {
                let key = session
                    .project
                    .clone()
                    .unwrap_or_else(|| "Other".to_owned());
                *counts.entry(key).or_default() += 1;
            }
        });
        sorted_counts(counts)
    });
    let visible = Memo::new(move |_| {
        let project = project.get();
        sessions.with(|s| {
            s.iter()
                .flatten()
                .filter(|session| {
                    project.is_empty() || session.project.as_deref().unwrap_or("Other") == project
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let busy = Memo::new(move |_| {
        sessions.with(|s| s.iter().flatten().filter(|s| s.status == "busy").count())
    });

    view! {
        <section class="page-section">
            <div class="section-heading-row">
                <h2 class="section-title">
                    "On the Mac"
                    {move || sessions.with(|s| s.as_ref().map(|s| view! { <span class="section-count">{s.len()}</span> }))}
                    {move || {
                        let busy = busy.get();
                        (busy > 0).then(|| view! { <span class="claude-busy-count">{format!("{busy} busy")}</span> })
                    }}
                </h2>
            </div>
            <div class="task-toolbar">
                <div class="task-filters" role="group" aria-label="Filter by project">
                    <button
                        type="button"
                        class=move || format!("chip-btn {}", active_if(project.with(String::is_empty)))
                        aria-pressed=move || project.with(String::is_empty).to_string()
                        on:click=move |_| project.set(String::new())
                    >
                        "All"
                    </button>
                    {move || project_chips(projects.get(), project)}
                </div>
                <button
                    type="button"
                    class=move || format!("chip-btn {}", active_if(include_stopped.get()))
                    aria-pressed=move || include_stopped.get().to_string()
                    on:click=move |_| include_stopped.update(|v| *v = !*v)
                >
                    "Show stopped"
                </button>
            </div>
            {move || {
                error
                    .get()
                    .map(|error| match error {
                        ClaudeLinkGetError::Link(link) => link_error_notice(&link).into_any(),
                        ClaudeLinkGetError::Client(client) => {
                            let prefix = if sessions.with(Option::is_some) { "Refresh failed: " } else { "" };
                            view! {
                                <div class="error-inline">
                                    {prefix}
                                    {error_message(client.message(), "Failed to load sessions")}
                                </div>
                            }
                                .into_any()
                        }
                    })
            }}
            {move || {
                (sessions.with(Option::is_none) && error.with(Option::is_none))
                    .then(|| view! { <div class="loading-inline">"Asking the Mac…"</div> })
            }}
            {move || {
                (sessions.with(Option::is_some) && visible.with(Vec::is_empty))
                    .then(|| {
                        view! {
                            <div class="muted">
                                {if include_stopped.get() { "No sessions." } else { "No running sessions." }}
                            </div>
                        }
                    })
            }}
            {move || {
                (!visible.with(Vec::is_empty))
                    .then(|| {
                        view! {
                            <div class="claude-session-grid">
                                <For
                                    each=move || visible.get()
                                    key=|session| format!("{}|{session:?}", session.session_id)
                                    children=move |session| session_card(session, now, on_open_transcript)
                                />
                            </div>
                        }
                    })
            }}
        </section>
    }
}

// ===== Action timeline =====

fn error_callout(error: &str) -> impl IntoView + use<> {
    let explained = explain_claude_error(Some(error));
    let warn = explained.code.as_deref() == Some("outcome_unknown");
    view! {
        <div class=format!("claude-error {}", if warn { "claude-error-warn" } else { "" })>
            {explained.code.map(|code| view! { <span class="claude-error-code">{code}</span> })}
            <span class="claude-error-message">{error.to_owned()}</span>
            {explained.hint.map(|hint| view! { <span class="claude-error-hint">{hint}</span> })}
        </div>
    }
}

/// UI state that must survive the 10 s activity polls.
#[derive(Clone, Copy)]
struct TimelineState {
    /// Group keys whose earlier actions are revealed.
    expanded_groups: RwSignal<HashSet<String>>,
    /// Call ids whose prompt text is unclamped.
    open_texts: RwSignal<HashSet<String>>,
}

impl TimelineState {
    fn text_open(self, call_id: &str) -> RwSignal<bool> {
        let id = call_id.to_owned();
        let open = RwSignal::new(self.open_texts.with_untracked(|s| s.contains(&id)));
        let texts = self.open_texts;
        Effect::new(move |_| {
            let value = open.get();
            texts.update(|set| {
                if value {
                    set.insert(id.clone());
                } else {
                    set.remove(&id);
                }
            });
        });
        open
    }
}

fn action_body(call: &McpCall, state: TimelineState) -> AnyView {
    let kind = claude_action_kind(&call.tool, &call.input);
    let input = &call.input;
    let output = &call.output;
    let session = output_session(call);
    let prompt = string_field(input, "prompt");
    let prompt_view = |prompt: Option<String>| {
        prompt.map(|p| expandable_text(p, "claude-quote", state.text_open(&call.call_id)))
    };
    match kind {
        ClaudeActionKind::Start => {
            let model = string_field(input, "model");
            let effort = string_field(input, "effort");
            let title = string_field(input, "title");
            let project = string_field(input, "project");
            let reused = boolean_field(output, "reused") == Some(true);
            view! {
                <div class="claude-action-tags">
                    {project.map(|p| view! { <span class="claude-project">{p}</span> })}
                    {model.map(|m| tag(m, None))}
                    {effort.map(|e| tag(format!("effort {e}"), None))}
                    {reused.then(|| tag("reused", Some("accent")))}
                </div>
                {title.map(|t| view! { <div class="claude-action-line">{t}</div> })}
                {prompt_view(prompt)}
            }
            .into_any()
        }
        ClaudeActionKind::Send => {
            let warning = string_field(output, "warning");
            let interrupt = boolean_field(input, "interrupt") == Some(true);
            view! {
                {interrupt.then(|| view! { <div class="claude-action-tags">{tag("interrupt", Some("warn"))}</div> })}
                {prompt_view(prompt)}
                {warning.map(|w| view! { <div class="claude-warning">{w}</div> })}
            }
            .into_any()
        }
        ClaudeActionKind::Wait => {
            let Some(timed_out) = boolean_field(output, "timedOut") else {
                return ().into_any();
            };
            let timeout = number_field(input, "timeoutSeconds");
            let status = session.as_ref().and_then(|s| s.status.clone());
            let revision = session.as_ref().and_then(|s| s.revision);
            if timed_out {
                view! {
                    <div class="claude-action-line">
                        {tag("timed out", Some("warn"))}
                        {timeout.map(|t| format!(" after {}s", number_string(t)))}
                        {status.map(|s| format!(", still {s}"))}
                    </div>
                }
                .into_any()
            } else {
                view! {
                    <div class="claude-action-line">
                        {tag("settled", Some("success"))}
                        {status.map(|s| format!(" {s}"))}
                        {revision.map(|r| format!(" at rev {}", number_string(r)))}
                    </div>
                }
                .into_any()
            }
        }
        ClaudeActionKind::Result => {
            if output.is_null() {
                return ().into_any();
            }
            match string_field(output, "result") {
                Some(result) => {
                    let truncated = boolean_field(output, "truncated") == Some(true);
                    view! {
                        <div class="claude-result">
                            <WorkspaceMarkdown content=result />
                            {truncated.then(|| tag("truncated", None))}
                        </div>
                    }
                    .into_any()
                }
                None => view! { <div class="claude-action-line muted">"No result yet."</div> }
                    .into_any(),
            }
        }
        ClaudeActionKind::Stop => ().into_any(),
        _ => view! { <div class="claude-action-line muted">{summarize_compact_action(call)}</div> }
            .into_any(),
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

fn timeline_item(call: McpCall, now: ReadSignal<f64>, state: TimelineState) -> impl IntoView {
    let kind = claude_action_kind(&call.tool, &call.input);
    let started = call.started_at as f64;
    let duration = call
        .duration_ms
        .map(|d| format!(" · {}", format_duration(d as f64)))
        .unwrap_or_default();
    view! {
        <li class=format!(
            "claude-tl-item claude-tl-{} claude-tl-status-{}",
            kind.as_str(),
            status_str(call.status),
        )>
            <span class="claude-tl-marker" aria-hidden="true"></span>
            <div class="claude-tl-content">
                <div class="claude-tl-head">
                    <span class="claude-tl-label">{kind.label()}</span>
                    {(call.status != McpCallStatus::Ok).then(|| view! { <CallStatusPill status=call.status /> })}
                    <span class="claude-tl-time" title=format_absolute(started)>
                        {move || format_relative_at(started, now.get())}
                        {duration}
                    </span>
                </div>
                {call.error.as_deref().filter(|e| !e.is_empty()).map(error_callout)}
                {action_body(&call, state)}
            </div>
        </li>
    }
}

fn action_group_card(
    group: ClaudeActionGroup,
    now: ReadSignal<f64>,
    on_open: OpenTranscript,
    state: TimelineState,
) -> impl IntoView {
    let key = group.key.clone();
    let show_all = {
        let key = key.clone();
        move || state.expanded_groups.with(|s| s.contains(&key))
    };
    let total = group.actions.len();
    let title = group.title.clone().unwrap_or_else(|| {
        if group.session_id.is_some() {
            "Untitled session".to_owned()
        } else {
            "Failed start".to_owned()
        }
    });
    let actions = group.actions.clone();
    let hidden_count = {
        let show_all = show_all.clone();
        move || {
            if show_all() {
                0
            } else {
                total.saturating_sub(TIMELINE_PAGE)
            }
        }
    };
    let hidden_for_list = hidden_count.clone();
    let open_title = title.clone();
    view! {
        <article class=format!("claude-group {}", if group.latest_failed { "claude-group-failed" } else { "" })>
            <header class="claude-group-head">
                <div class="claude-group-title-row">
                    <h3 class="claude-group-title">{title}</h3>
                    {session_status(group.status.clone())}
                </div>
                <div class="claude-group-meta meta-row muted">
                    {group.project.clone().map(|p| view! { <span class="claude-project">{p}</span> })}
                    {group
                        .session_id
                        .clone()
                        .map(|id| view! { <span class="claude-mono" title=id.clone()>{short_session_id(&id)}</span> })}
                    <span>{format!("{total} action{}", if total == 1 { "" } else { "s" })}</span>
                    <span title=format_absolute(group.latest_at as f64)>
                        {move || format_relative_at(group.latest_at as f64, now.get())}
                    </span>
                    {group
                        .session_id
                        .clone()
                        .map(|id| {
                            view! {
                                <button
                                    type="button"
                                    class="claude-link-btn"
                                    on:click=move |_| on_open.run((id.clone(), open_title.clone()))
                                >
                                    "Transcript"
                                </button>
                            }
                        })}
                </div>
            </header>
            {move || {
                let hidden = hidden_count();
                (hidden > 0)
                    .then(|| {
                        let key = key.clone();
                        view! {
                            <button
                                type="button"
                                class="claude-more-btn"
                                on:click=move |_| state.expanded_groups.update(|s| { s.insert(key.clone()); })
                            >
                                {format!("Show {hidden} earlier action{}", if hidden == 1 { "" } else { "s" })}
                            </button>
                        }
                    })
            }}
            <ol class="claude-timeline">
                {move || {
                    actions
                        .iter()
                        .skip(hidden_for_list())
                        .cloned()
                        .map(|call| timeline_item(call, now, state))
                        .collect_view()
                }}
            </ol>
        </article>
    }
}

fn other_actions(
    calls: Vec<McpCall>,
    now: ReadSignal<f64>,
    show_all: RwSignal<bool>,
) -> impl IntoView {
    let total = calls.len();
    let rows = move || {
        let limit = if show_all.get() { total } else { 8 };
        calls
            .iter()
            .take(limit)
            .map(|call| {
                let kind = claude_action_kind(&call.tool, &call.input);
                let started = call.started_at as f64;
                let summary = match call.error.clone().filter(|e| !e.is_empty()) {
                    Some(error) => view! { <span class="claude-other-error">{error}</span> }.into_any(),
                    None => summarize_compact_action(call).into_any(),
                };
                let status = call.status;
                view! {
                    <li class="claude-other-row">
                        <span class="claude-other-label">{kind.label()}</span>
                        <span class="claude-other-summary">{summary}</span>
                        {(status != McpCallStatus::Ok).then(|| view! { <CallStatusPill status=status /> })}
                        <span class="claude-tl-time" title=format_absolute(started)>
                            {move || format_relative_at(started, now.get())}
                        </span>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <article class="claude-group claude-group-other">
            <header class="claude-group-head">
                <div class="claude-group-title-row">
                    <h3 class="claude-group-title">"Other"</h3>
                    <span class="muted claude-group-sub">"Listings and status checks"</span>
                </div>
            </header>
            <ul class="claude-other-list">{rows}</ul>
            {move || {
                let shown = if show_all.get() { total } else { total.min(8) };
                (!show_all.get() && total > shown)
                    .then(|| {
                        view! {
                            <button type="button" class="claude-more-btn" on:click=move |_| show_all.set(true)>
                                {format!("Show {} more", total - shown)}
                            </button>
                        }
                    })
            }}
        </article>
    }
}

#[component]
fn ActionsSection(
    #[prop(into)] actions: Signal<Vec<McpCall>>,
    now: ReadSignal<f64>,
    on_open_transcript: OpenTranscript,
) -> impl IntoView {
    let project = RwSignal::new(String::new());
    let state = TimelineState {
        expanded_groups: RwSignal::new(HashSet::new()),
        open_texts: RwSignal::new(HashSet::new()),
    };
    let show_all_other = RwSignal::new(false);
    let grouped = Memo::new(move |_| actions.with(|a| group_claude_actions(a)));
    let projects = Memo::new(move |_| {
        let mut counts = HashMap::<String, usize>::new();
        grouped.with(|g| {
            for group in &g.sessions {
                if let Some(project) = group.project.clone().filter(|p| !p.is_empty()) {
                    *counts.entry(project).or_default() += 1;
                }
            }
        });
        sorted_counts(counts)
    });
    let visible = Memo::new(move |_| {
        let project = project.get();
        grouped.with(|g| {
            g.sessions
                .iter()
                .filter(|group| {
                    project.is_empty() || group.project.as_deref() == Some(project.as_str())
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let other = Memo::new(move |_| grouped.with(|g| g.other.clone()));

    view! {
        <section class="page-section">
            <div class="section-heading-row">
                <h2 class="section-title">
                    "Actions " <span class="section-count">{move || actions.with(Vec::len)}</span>
                </h2>
                <Link to="/mcp-activity" class="section-view-all">"All MCP calls ›"</Link>
            </div>
            {move || {
                let projects = projects.get();
                (!projects.is_empty())
                    .then(|| {
                        view! {
                            <div class="task-filters claude-filter-row" role="group" aria-label="Filter by project">
                                <button
                                    type="button"
                                    class=move || format!("chip-btn {}", active_if(project.with(String::is_empty)))
                                    aria-pressed=move || project.with(String::is_empty).to_string()
                                    on:click=move |_| project.set(String::new())
                                >
                                    "All"
                                </button>
                                {project_chips(projects, project)}
                            </div>
                        }
                    })
            }}
            {move || {
                if actions.with(Vec::is_empty) {
                    view! { <div class="muted">"No Claude Code actions recorded yet."</div> }.into_any()
                } else {
                    view! {
                        <div class="claude-groups">
                            <For
                                each=move || visible.get()
                                key=|group| format!("{}|{group:?}", group.key)
                                children=move |group| action_group_card(group, now, on_open_transcript, state)
                            />
                            {move || {
                                let other = other.get();
                                (project.with(String::is_empty) && !other.is_empty())
                                    .then(|| other_actions(other, now, show_all_other))
                            }}
                        </div>
                    }
                        .into_any()
                }
            }}
        </section>
    }
}

// ===== Page =====

#[component]
pub fn ClaudePage() -> impl IntoView {
    let now = use_now(15_000);
    let activity = RwSignal::new(None::<ClaudeActivityResponse>);
    let error = RwSignal::new(None::<String>);
    let transcript = RwSignal::new(None::<(String, String)>);

    use_visible_poll(
        Signal::stored("activity".to_owned()),
        || api::fetch_claude_activity(200),
        move |response: ClaudeActivityResponse| {
            activity.set(Some(response));
            error.set(None);
        },
        move |err: ApiClientError| {
            error.set(Some(error_message(
                err.message(),
                "Failed to load Claude activity",
            )));
        },
        POLL,
    );

    let open_transcript: OpenTranscript =
        Callback::new(move |(session_id, title): (String, String)| {
            transcript.set(Some((session_id, title)))
        });
    let has_activity = Memo::new(move |_| activity.with(Option::is_some));
    let link = Memo::new(move |_| activity.with(|a| a.as_ref().map(|a| a.link.clone())));
    let actions = Signal::derive(move || {
        activity.with(|a| a.as_ref().map(|a| a.actions.clone()).unwrap_or_default())
    });

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"Claude Code"</h1>
                <p class="page-subtitle">"Sessions on the MacBook and what agents asked them to do."</p>
            </div>
        </div>

        {move || {
            (!has_activity.get() && error.with(Option::is_none))
                .then(|| view! { <div class="loading">"Loading…"</div> })
        }}
        {move || {
            error
                .get()
                .filter(|_| !has_activity.get())
                .map(|e| {
                    view! {
                        <div class="error">
                            <div>"Failed to load Claude activity"</div>
                            <div class="error-detail">{e}</div>
                        </div>
                    }
                })
        }}
        {move || {
            error
                .get()
                .filter(|_| has_activity.get())
                .map(|e| {
                    view! {
                        <div class="error-inline stale-note">
                            {format!("Refresh failed ({e}), showing last known state.")}
                        </div>
                    }
                })
        }}
        {move || link.get().map(|link| link_hero(link, now))}

        <SessionsSection now=now on_open_transcript=open_transcript />

        {move || {
            has_activity
                .get()
                .then(|| view! { <ActionsSection actions=actions now=now on_open_transcript=open_transcript /> })
        }}

        {move || {
            transcript
                .get()
                .map(|(session_id, title)| {
                    view! {
                        <TranscriptModal
                            session_id=session_id
                            title=title
                            on_close=Callback::new(move |()| transcript.set(None))
                        />
                    }
                })
        }}
    }
}
