//! Task-run log modal. Finished runs load their
//! persisted logs once; a running task tails the per-run SSE stream until its
//! `done` frame. Reconnects are safe: `init` replaces the line state.

use leptos::html::Div;
use leptos::prelude::*;
use omni_api::runs::{LogLevel, Run, RunLogLine, RunLogStreamFrame, RunStatus};

use super::badges::{StatusDot, TriggerBadge};
use super::task_card::run_duration;
use crate::api;
use crate::hooks::{use_modal, use_now};
use crate::sse::{SseConnection, SseMessage};
use crate::task::spawn_scoped;
use crate::utils::download::download_file;
use crate::utils::format::{format_absolute, to_title_case};
use crate::utils::js::{iso_string, local_time_parts};

const LEVELS: [LogLevel; 4] = [
    LogLevel::Debug,
    LogLevel::Info,
    LogLevel::Warn,
    LogLevel::Error,
];
const STICK_THRESHOLD_PX: i32 = 48;

pub fn level_str(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

/// `HH:MM:SS.mmm` local time.
pub fn format_log_time(t: f64) -> String {
    let (h, m, s, ms) = local_time_parts(t);
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// "Main:LiveCheck:Twitch" → "LiveCheck:Twitch"; a bare root name stays.
pub fn short_logger_name(name: &str) -> String {
    match name.split_once(':') {
        Some((_, rest)) if !rest.is_empty() => rest.to_owned(),
        _ => name.to_owned(),
    }
}

/// Shared log-line rendering (task-run viewer and email log modal).
#[component]
pub fn LogLines(#[prop(into)] lines: Signal<Vec<RunLogLine>>) -> impl IntoView {
    // Lines are append-only, so a positional key is stable.
    view! {
        <For
            each=move || lines.get().into_iter().enumerate()
            key=|(index, line)| (*index, line.t)
            children=|(_, line)| {
                let level = level_str(line.level);
                view! {
                    <div class=format!("log-line log-line-{level}")>
                        <span class="log-line-meta">
                            <span class="log-time">{format_log_time(line.t as f64)}</span>
                            <span class=format!("log-level log-level-{level}")>{level}</span>
                            <span class="log-logger">{short_logger_name(&line.logger)}</span>
                        </span>
                        <span class="log-msg">{line.msg}</span>
                    </div>
                }
            }
        />
    }
}

/// The downloaded `.log` text for `lines`.
pub fn log_file_content(lines: &[RunLogLine]) -> String {
    let body = lines
        .iter()
        .map(|line| {
            format!(
                "{} {:<5} {} {}",
                iso_string(line.t as f64),
                level_str(line.level).to_uppercase(),
                line.logger,
                line.msg
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}\n")
}

#[component]
pub fn LogViewer(run: Run, on_close: Callback<()>) -> impl IntoView {
    let run_id = run.run_id.clone();
    let started_running = run.status == RunStatus::Running;
    let current = RwSignal::new(run);
    let lines = RwSignal::new(None::<Vec<RunLogLine>>);
    let dropped = RwSignal::new(0u64);
    let error = RwSignal::new(None::<String>);
    let live = RwSignal::new(started_running);
    let hidden_levels = RwSignal::new(vec![LogLevel::Debug]);
    let now = use_now(1000);
    let body_ref = NodeRef::<Div>::new();
    let stick_to_bottom = StoredValue::new(true);

    if started_running {
        let url = api::run_log_stream_url(&run_id);
        spawn_scoped(async move {
            let Some(mut connection) = SseConnection::open(&url, &["init", "line", "done"]) else {
                return;
            };
            while let Some(message) = connection.next().await {
                let SseMessage::Event { name, data } = message else {
                    continue;
                };
                match RunLogStreamFrame::decode(&name, &data) {
                    Some(Ok(RunLogStreamFrame::Init(data))) => {
                        current.set(data.run);
                        lines.set(Some(data.lines));
                        dropped.set(data.dropped);
                        error.set(None);
                    }
                    Some(Ok(RunLogStreamFrame::Line(line))) => lines.update(|lines| match lines {
                        Some(lines) => lines.push(line),
                        None => *lines = Some(vec![line]),
                    }),
                    Some(Ok(RunLogStreamFrame::Done(run))) => {
                        current.set(run);
                        live.set(false);
                        break;
                    }
                    Some(Err(decode)) => error.set(Some(decode.to_string())),
                    None => {}
                }
            }
        });
    } else {
        spawn_scoped(async move {
            match api::fetch_run_logs(&run_id).await {
                Ok(data) => {
                    current.set(data.run);
                    lines.set(Some(data.lines));
                    dropped.set(data.dropped);
                }
                Err(e) => error.set(Some(e.message().to_owned())),
            }
        });
    }

    let modal_ref = use_modal(move || on_close.run(()));

    let line_count = Memo::new(move |_| lines.with(|l| l.as_ref().map_or(0, Vec::len)));
    let visible = Memo::new(move |_| {
        let hidden = hidden_levels.get();
        lines.with(|l| {
            l.as_ref()
                .map(|l| {
                    l.iter()
                        .filter(|line| !hidden.contains(&line.level))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let hidden_count = move || line_count.get().saturating_sub(visible.with(Vec::len));

    // Follow the tail only while the user hasn't scrolled up.
    Effect::new(move |_| {
        line_count.track();
        hidden_levels.track();
        if let Some(body) = body_ref.get()
            && stick_to_bottom.get_value()
        {
            body.set_scroll_top(body.scroll_height());
        }
    });
    let on_scroll = move |_| {
        if let Some(body) = body_ref.get() {
            stick_to_bottom.set_value(
                body.scroll_height() - body.scroll_top() - body.client_height()
                    < STICK_THRESHOLD_PX,
            );
        }
    };
    let toggle_level = move |level: LogLevel| {
        hidden_levels.update(|hidden| {
            if let Some(position) = hidden.iter().position(|l| *l == level) {
                hidden.remove(position);
            } else {
                hidden.push(level);
            }
        });
    };
    let download = move |_| {
        let content = log_file_content(&visible.get_untracked());
        // runId is "TaskName:startedAt:seq"; colons are unfriendly in filenames.
        let name = current.with_untracked(|r| r.run_id.replace(':', "-"));
        download_file(&format!("{name}.log"), &content, "text/plain");
    };

    let level_chips = LEVELS
        .into_iter()
        .map(|level| {
            let count = move || {
                lines.with(|l| {
                    l.as_ref()
                        .map_or(0, |l| l.iter().filter(|line| line.level == level).count())
                })
            };
            view! {
                <button
                    type="button"
                    class=move || {
                        format!(
                            "chip-btn log-level-chip {}",
                            if hidden_levels.with(|h| h.contains(&level)) { "" } else { "active" },
                        )
                    }
                    on:click=move |_| toggle_level(level)
                >
                    {level_str(level)}
                    <span class="log-level-count">{count}</span>
                </button>
            }
        })
        .collect_view();

    let body_status = move || {
        let err = error.get();
        let loaded = lines.with(Option::is_some);
        let count = line_count.get();
        let is_live = live.get();
        if let Some(err) = err {
            return Some(
                view! { <div class="error-inline">"Failed to load logs: " {err}</div> }.into_any(),
            );
        }
        if !loaded {
            return Some(view! { <div class="loading-inline">"Loading logs…"</div> }.into_any());
        }
        if count == 0 && !is_live {
            return Some(
                view! { <div class="muted log-empty">"No logs were captured for this run."</div> }
                    .into_any(),
            );
        }
        if count == 0 {
            return Some(
                view! { <div class="muted log-empty">"Waiting for output…"</div> }.into_any(),
            );
        }
        None
    };
    let all_hidden = move || {
        let count = line_count.get();
        (count > 0 && visible.with(Vec::is_empty)).then(|| {
            view! {
                <div class="muted log-empty">
                    {format!("All {count} lines are hidden by the level filter.")}
                </div>
            }
        })
    };

    view! {
        <div class="modal-root">
            <button
                type="button"
                class="modal-backdrop"
                tabindex="-1"
                on:click=move |_| on_close.run(())
                aria-label="Close log viewer"
            ></button>
            <div
                class="log-modal"
                node_ref=modal_ref
                tabindex="-1"
                aria-modal="true"
                role="dialog"
                aria-label=move || format!("Logs for {}", current.with(|r| r.task_name.clone()))
            >
                <div class="log-modal-header">
                    <div class="log-modal-title">
                        {move || {
                            let run = current.get();
                            view! {
                                <StatusDot status=run.status/>
                                <span class="log-modal-task">{to_title_case(&run.task_name)}</span>
                                <TriggerBadge trigger=run.trigger/>
                            }
                        }}
                        {move || live.get().then(|| view! { <span class="log-live-badge">"live"</span> })}
                    </div>
                    <div class="log-modal-meta meta-row muted">
                        <span>{move || format_absolute(current.with(|r| r.started_at) as f64)}</span>
                        <span>{move || current.with(|r| run_duration(r, now.get()))}</span>
                    </div>
                    <button
                        type="button"
                        class="log-modal-close"
                        on:click=move |_| on_close.run(())
                        aria-label="Close"
                    >
                        "✕"
                    </button>
                </div>
                {move || {
                    current
                        .with(|r| r.error.clone())
                        .map(|e| view! { <div class="run-error log-modal-error">{e}</div> })
                }}
                <div class="log-modal-controls">
                    {level_chips}
                    <button
                        type="button"
                        class="log-download-btn"
                        disabled=move || visible.with(Vec::is_empty)
                        title=move || {
                            if hidden_count() > 0 {
                                "Download the visible lines (level filter applied)"
                            } else {
                                "Download all lines"
                            }
                        }
                        on:click=download
                    >
                        "Download"
                    </button>
                    {move || {
                        let dropped = dropped.get();
                        (dropped > 0)
                            .then(|| {
                                view! {
                                    <span class="muted log-dropped-note">
                                        {format!("{dropped} oldest lines dropped")}
                                    </span>
                                }
                            })
                    }}
                </div>
                <div class="log-modal-body" node_ref=body_ref on:scroll=on_scroll>
                    {body_status}
                    {all_hidden}
                    <LogLines lines=visible/>
                </div>
                {move || {
                    let hidden = hidden_count();
                    (hidden > 0 && !visible.with(Vec::is_empty))
                        .then(|| {
                            view! {
                                <div class="log-modal-footer muted">
                                    {format!("{hidden} lines hidden by level filter")}
                                </div>
                            }
                        })
                }}
            </div>
        </div>
    }
}
