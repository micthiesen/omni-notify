//! Task-run logs: [`use_run_logs`] (finished runs load once; running runs
//! tail the per-run SSE stream until `done`, and `init` replaces the lines so
//! reconnects are safe), the inline [`LogWell`] and the full-screen
//! [`LogViewer`] with level filters and download.

use leptos::html::Div;
use leptos::prelude::*;
use omni_api::runs::{LogLevel, Run, RunLogLine, RunLogStreamFrame, RunStatus};

use super::badges::{Status, TriggerBadge, run_status_kind};
use super::button::{Button, ButtonSize, ButtonVariant};
use super::controls::Chip;
use super::icon::Icon;
use super::inspector::Modal;
use super::task_display::run_duration;
use crate::api;
use crate::hooks::use_now;
use crate::sse::{SseConnection, SseMessage};
use crate::task::spawn_scoped;
use crate::utils::download::download_file;
use crate::utils::format::{format_absolute, task_label};
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

/// Live log state of one run.
#[derive(Clone, Copy)]
pub struct RunLogs {
    pub run: RwSignal<Run>,
    pub lines: RwSignal<Option<Vec<RunLogLine>>>,
    pub dropped: RwSignal<u64>,
    pub error: RwSignal<Option<String>>,
    /// The SSE tail is open.
    pub streaming: RwSignal<bool>,
}

async fn load_stored_logs(logs: RunLogs, run_id: &str) {
    match api::fetch_run_logs(run_id).await {
        Ok(data) => {
            logs.run.set(data.run);
            logs.lines.set(Some(data.lines));
            logs.dropped.set(data.dropped);
        }
        Err(e) => logs.error.set(Some(e.message().to_owned())),
    }
}

/// Loads (or tails) the logs of `run` under the current owner.
pub fn use_run_logs(run: Run) -> RunLogs {
    let run_id = run.run_id.clone();
    let started_running = run.status == RunStatus::Running;
    let logs = RunLogs {
        run: RwSignal::new(run),
        lines: RwSignal::new(None),
        dropped: RwSignal::new(0),
        error: RwSignal::new(None),
        streaming: RwSignal::new(started_running),
    };
    if started_running {
        let url = api::run_log_stream_url(&run_id);
        spawn_scoped(async move {
            let Some(mut connection) = SseConnection::open(&url, &["init", "line", "done"]) else {
                logs.streaming.set(false);
                return;
            };
            while let Some(message) = connection.next().await {
                let (name, data) = match message {
                    SseMessage::Event { name, data } => (name, data),
                    SseMessage::Error { closed: false } => continue,
                    // Closed for good (server refused it, or the page was
                    // hidden): fall back to the stored log once.
                    SseMessage::Error { closed: true } => {
                        drop(connection);
                        logs.streaming.set(false);
                        load_stored_logs(logs, &run_id).await;
                        return;
                    }
                };
                match RunLogStreamFrame::decode(&name, &data) {
                    Some(Ok(RunLogStreamFrame::Init(data))) => {
                        logs.run.set(data.run);
                        logs.lines.set(Some(data.lines));
                        logs.dropped.set(data.dropped);
                        logs.error.set(None);
                    }
                    Some(Ok(RunLogStreamFrame::Line(line))) => {
                        logs.lines.update(|lines| match lines {
                            Some(lines) => lines.push(line),
                            None => *lines = Some(vec![line]),
                        });
                    }
                    Some(Ok(RunLogStreamFrame::Done(run))) => {
                        logs.run.set(run);
                        logs.streaming.set(false);
                        break;
                    }
                    Some(Err(decode)) => logs.error.set(Some(decode.to_string())),
                    None => {}
                }
            }
        });
    } else {
        spawn_scoped(async move { load_stored_logs(logs, &run_id).await });
    }
    logs
}

/// Time, level, logger and message rows (shared by run and email logs).
#[component]
pub fn LogLines(#[prop(into)] lines: Signal<Vec<RunLogLine>>) -> impl IntoView {
    // Lines are append-only, so a positional key is stable.
    view! {
        <div class="log-lines">
            <For
                each=move || lines.get().into_iter().enumerate()
                key=|(index, line)| (*index, line.t)
                children=|(_, line)| {
                    let level = level_str(line.level);
                    view! {
                        <div class=format!("log-line {level}")>
                            <span class="log-t">{format_log_time(line.t as f64)}</span>
                            <span class="log-lv">{level}</span>
                            <span class="log-src" title=line.logger.clone()>{short_logger_name(&line.logger)}</span>
                            <span class="log-msg">{line.msg}</span>
                        </div>
                    }
                }
            />
        </div>
    }
}

/// Keeps a scroll container pinned to the bottom while the reader has not
/// scrolled up.
fn use_follow_tail(body: NodeRef<Div>, trigger: Signal<usize>) -> impl Fn(web_sys::Event) + Copy {
    let stick = StoredValue::new(true);
    Effect::new(move |_| {
        trigger.track();
        if let Some(body) = body.get()
            && stick.get_value()
        {
            body.set_scroll_top(body.scroll_height());
        }
    });
    move |_| {
        if let Some(body) = body.get_untracked() {
            stick.set_value(
                body.scroll_height() - body.scroll_top() - body.client_height()
                    < STICK_THRESHOLD_PX,
            );
        }
    }
}

fn status_note(logs: RunLogs, visible: usize) -> Option<AnyView> {
    if let Some(err) = logs.error.get() {
        return Some(
            view! { <p class="log-note warn">{format!("Could not load the logs: {err}")}</p> }
                .into_any(),
        );
    }
    let count = logs.lines.with(|l| l.as_ref().map(Vec::len))?;
    let streaming = logs.streaming.get();
    if count == 0 && !streaming {
        return Some(
            view! { <p class="log-note">"No log lines were captured for this run."</p> }.into_any(),
        );
    }
    if count == 0 && streaming {
        return Some(
            view! { <p class="log-note" role="status">"No log output yet. New lines appear here as the run writes them."</p> }
                .into_any(),
        );
    }
    if count > 0 && visible == 0 {
        return Some(
            view! { <p class="log-note">{format!("All {count} lines are hidden by the level filter.")}</p> }
                .into_any(),
        );
    }
    None
}

/// Inline log well (inspector). Mount it keyed by run id.
#[component]
pub fn LogWell(
    run: Run,
    /// Cap the height at 420 px and scroll inside.
    #[prop(optional)]
    bounded: bool,
) -> impl IntoView {
    let logs = use_run_logs(run);
    let body = NodeRef::<Div>::new();
    let all = Signal::derive(move || logs.lines.get().unwrap_or_default());
    let count = Signal::derive(move || logs.lines.with(|l| l.as_ref().map_or(0, Vec::len)));
    let on_scroll = use_follow_tail(body, count);
    view! {
        <div
            class=if bounded { "logwell bounded" } else { "logwell" }
            node_ref=body
            on:scroll=on_scroll
            role="log"
            aria-label="Run log"
        >
            {move || {
                (logs.lines.with(Option::is_none) && logs.error.with(Option::is_none)).then(|| view! {
                    <p class="log-note" role="status">"Loading log…"</p>
                })
            }}
            {move || {
                let dropped = logs.dropped.get();
                (dropped > 0).then(|| view! { <p class="log-note warn">{format!("{dropped} oldest lines were dropped.")}</p> })
            }}
            {move || status_note(logs, count.get())}
            <LogLines lines=all/>
            {move || logs.streaming.get().then(|| view! { <p class="log-cursor">"streaming"</p> })}
        </div>
    }
}

/// Full-screen log viewer with level filters (debug hidden by default) and
/// download of the visible lines.
#[component]
pub fn LogViewer(run: Run, on_close: Callback<()>) -> impl IntoView {
    let logs = use_run_logs(run);
    let hidden_levels = RwSignal::new(vec![LogLevel::Debug]);
    let now = use_now(1000);
    let body = NodeRef::<Div>::new();

    let line_count = Memo::new(move |_| logs.lines.with(|l| l.as_ref().map_or(0, Vec::len)));
    let visible = Memo::new(move |_| {
        let hidden = hidden_levels.get();
        logs.lines.with(|l| {
            l.as_ref()
                .map(|l| {
                    l.iter()
                        .filter(|line| !hidden.contains(&line.level))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let visible_count = Signal::derive(move || visible.with(Vec::len));
    let trigger = Signal::derive(move || line_count.get() + hidden_levels.with(Vec::len));
    let on_scroll = use_follow_tail(body, trigger);
    let hidden_count = move || line_count.get().saturating_sub(visible.with(Vec::len));
    let toggle_level = move |level: LogLevel| {
        hidden_levels.update(|hidden| {
            if let Some(position) = hidden.iter().position(|l| *l == level) {
                hidden.remove(position);
            } else {
                hidden.push(level);
            }
        });
    };
    let download = Callback::new(move |_| {
        let content = log_file_content(&visible.get_untracked());
        // runId is "TaskName:startedAt:seq"; colons are unfriendly in filenames.
        let name = logs.run.with_untracked(|r| r.run_id.replace(':', "-"));
        download_file(&format!("{name}.log"), &content, "text/plain");
    });
    let chips = LEVELS
        .into_iter()
        .map(|level| {
            let count = Signal::derive(move || {
                logs.lines.with(|l| {
                    l.as_ref()
                        .map_or(0, |l| l.iter().filter(|line| line.level == level).count())
                })
            });
            view! {
                <Chip
                    pressed=Signal::derive(move || !hidden_levels.with(|h| h.contains(&level)))
                    on_click=Callback::new(move |()| toggle_level(level))
                    count=count
                >
                    {level_str(level)}
                </Chip>
            }
        })
        .collect_view();
    let head = ViewFn::from(move || {
        let run = logs.run.get();
        view! {
            <div class="cluster">
                <Status kind=run_status_kind(run.status)/>
                <TriggerBadge trigger=run.trigger/>
                {move || logs.streaming.get().then(|| view! { <span class="tag signal">"streaming"</span> })}
            </div>
            <h2>{task_label(&run.task_name, None)}</h2>
            <p class="small muted dot-sep">
                <span>{format_absolute(run.started_at as f64)}</span>
                <span class="num">{move || logs.run.with(|r| run_duration(r, now.get()))}</span>
            </p>
        }
    });
    view! {
        <Modal label=Signal::derive(move || format!("Logs for {}", logs.run.with(|r| r.task_name.clone()))) on_close head>
            {move || logs.run.with(|r| r.error.clone()).map(|e| view! {
                <p class="log-note warn" style="border-bottom: 1px solid var(--line)">{e}</p>
            })}
            <div class="log-toolbar">
                <div class="chips" role="group" aria-label="Log levels">{chips}</div>
                <span class="spacer"></span>
                {move || {
                    let hidden = hidden_count();
                    (hidden > 0).then(|| view! { <span class="small muted">{format!("{hidden} hidden")}</span> })
                }}
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    icon=Icon::Download
                    disabled=Signal::derive(move || visible.with(Vec::is_empty))
                    disabled_reason="No visible lines to download"
                    on_click=download
                >
                    "Download"
                </Button>
            </div>
            <div class="logwell fill" node_ref=body on:scroll=on_scroll role="log">
                {move || (logs.lines.with(Option::is_none) && logs.error.with(Option::is_none)).then(|| view! {
                    <p class="log-note" role="status">"Loading log…"</p>
                })}
                {move || {
                    let dropped = logs.dropped.get();
                    (dropped > 0).then(|| view! { <p class="log-note warn">{format!("{dropped} oldest lines were dropped.")}</p> })
                }}
                {move || status_note(logs, visible_count.get())}
                <LogLines lines=Signal::derive(move || visible.get())/>
                {move || logs.streaming.get().then(|| view! { <p class="log-cursor">"streaming"</p> })}
            </div>
        </Modal>
    }
}
