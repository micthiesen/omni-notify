//! One scheduled task with its next run, last run and history accordion
//! (`components/TaskCard.tsx`).

use leptos::prelude::*;
use omni_api::runs::Run;
use omni_api::tasks::TaskInfo;

use super::badges::{StatusDot, TriggerBadge, status_str};
use crate::api;
use crate::hooks::use_now;
use crate::task::spawn_scoped;
use crate::utils::cron::describe_cron;
use crate::utils::format::{
    format_absolute, format_countdown, format_duration, format_relative, task_label,
};
use crate::utils::js::{now_ms, parse_date_ms};

const HISTORY_LIMIT: usize = 10;

/// Elapsed time of a run (until now while it is still running).
pub fn run_duration(run: &Run, now: f64) -> String {
    let end = run.finished_at.map_or(now, |f| f as f64);
    format_duration(end - run.started_at as f64)
}

/// [`run_duration`] against the current time.
pub fn run_duration_now(run: &Run) -> String {
    run_duration(run, now_ms())
}

#[component]
fn LastRunSummary(run: Option<Run>, on_view_logs: Callback<Run>) -> impl IntoView {
    let Some(run) = run else {
        return view! { <div class="task-last-run muted">"No runs yet"</div> }.into_any();
    };
    let clicked = run.clone();
    let status = status_str(run.status);
    let detail = match (&run.error, &run.summary) {
        (Some(error), _) => Some(view! { <div class="run-error">{error.clone()}</div> }.into_any()),
        (None, Some(summary)) => {
            Some(view! { <div class="run-summary">{summary.clone()}</div> }.into_any())
        }
        _ => None,
    };
    view! {
        <button
            type="button"
            class="task-last-run row-btn"
            on:click=move |_| on_view_logs.run(clicked.clone())
            title="View logs"
        >
            <div class="task-last-run-meta">
                <StatusDot status=run.status/>
                <span class=format!("run-status-text run-status-{status}")>{status}</span>
                <span title=format_absolute(run.started_at as f64)>
                    {format_relative(run.started_at as f64)}
                </span>
                <span class="muted">{run_duration_now(&run)}</span>
                <TriggerBadge trigger=run.trigger/>
            </div>
            {detail}
        </button>
    }
    .into_any()
}

#[component]
fn HistoryList(runs: Vec<Run>, on_view_logs: Callback<Run>) -> impl IntoView {
    if runs.is_empty() {
        return view! { <div class="muted history-empty">"No earlier runs."</div> }.into_any();
    }
    let rows = runs
        .into_iter()
        .map(|run| {
            let clicked = run.clone();
            let detail = run
                .error
                .clone()
                .map(|e| ("run-error", e))
                .or_else(|| run.summary.clone().map(|s| ("run-summary", s)));
            view! {
                <button
                    type="button"
                    class="history-row row-btn"
                    on:click=move |_| on_view_logs.run(clicked.clone())
                    title="View logs"
                >
                    <StatusDot status=run.status/>
                    <span class="history-time" title=format_absolute(run.started_at as f64)>
                        {format_relative(run.started_at as f64)}
                    </span>
                    <span class="muted">{run_duration_now(&run)}</span>
                    <TriggerBadge trigger=run.trigger/>
                    {detail.map(|(class, text)| {
                        view! { <span class=format!("history-detail {class}")>{text}</span> }
                    })}
                </button>
            }
        })
        .collect_view();
    view! { <div class="history-list">{rows}</div> }.into_any()
}

/// `task` updates live from the snapshot; the expanded history refetches
/// whenever a new run appears or the current one finishes.
#[component]
pub fn TaskCard(
    #[prop(into)] task: Signal<TaskInfo>,
    on_run: Callback<String>,
    on_view_logs: Callback<Run>,
) -> impl IntoView {
    let now = use_now(1000);
    let expanded = RwSignal::new(false);
    let history_id = format!("task-history-{}", task.with_untracked(|t| t.name.clone()));
    let history = RwSignal::new(None::<Vec<Run>>);
    let history_error = RwSignal::new(None::<String>);

    let name = Memo::new(move |_| task.with(|t| t.name.clone()));
    let last_run_id =
        Memo::new(move |_| task.with(|t| t.last_run.as_ref().map(|r| r.run_id.clone())));
    let last_run_finished =
        Memo::new(move |_| task.with(|t| t.last_run.as_ref().and_then(|r| r.finished_at)));

    Effect::new(move |_| {
        if !expanded.get() {
            return;
        }
        let name = name.get();
        last_run_id.track();
        last_run_finished.track();
        // One extra: the newest run already shows in the last-run line.
        spawn_scoped(async move {
            match api::fetch_task_runs(Some(&name), Some((HISTORY_LIMIT + 1) as u32)).await {
                Ok(data) => {
                    history.set(Some(data.runs));
                    history_error.set(None);
                }
                Err(error) => history_error.set(Some(error.message().to_owned())),
            }
        });
    });

    let header_dot = move || {
        task.with(|t| {
            if t.running {
                view! { <span class="running-pulse" title="Running"></span> }.into_any()
            } else {
                let (status, title) = match &t.last_run {
                    Some(run) => (
                        status_str(run.status),
                        format!("Last run: {}", status_str(run.status)),
                    ),
                    None => ("none", "No runs yet".to_owned()),
                };
                view! { <span class=format!("status-dot status-{status}") title=title></span> }
                    .into_any()
            }
        })
    };
    let next_run = move || {
        let next_ms = task.with(|t| t.next_runs.first().and_then(|iso| parse_date_ms(iso)));
        match next_ms {
            Some(next_ms) => view! {
                <span class="next-run-value meta-row" title=format_absolute(next_ms)>
                    <span>{move || format_countdown(next_ms - now.get())}</span>
                    <span class="muted">{format_absolute(next_ms)}</span>
                </span>
            }
            .into_any(),
            None => view! { <span class="muted">"Not Scheduled"</span> }.into_any(),
        }
    };
    let schedule = move || {
        task.with(|t| {
            let human = describe_cron(&t.schedule);
            view! {
                <code class="cron-raw">{t.schedule.clone()}</code>
                {human.map(|h| view! { <span class="cron-human">{h}</span> })}
            }
        })
    };
    let last_run = move || {
        let run = task.with(|t| t.last_run.clone());
        view! { <LastRunSummary run on_view_logs/> }
    };
    let history_panel_id = history_id.clone();
    let history_panel = move || {
        expanded.get().then(|| {
            let body = move || {
                let runs = history.get();
                let error = history_error.get();
                let loading = (runs.is_none() && error.is_none())
                    .then(|| view! { <div class="muted history-empty">"Loading…"</div> });
                let error_view =
                    error.map(|e| view! { <div class="error-inline history-empty">{e}</div> });
                let list = runs.map(|runs| {
                    let newest = last_run_id.get_untracked();
                    let runs: Vec<Run> = runs
                        .into_iter()
                        .filter(|run| Some(&run.run_id) != newest.as_ref())
                        .take(HISTORY_LIMIT)
                        .collect();
                    view! { <HistoryList runs on_view_logs/> }
                });
                view! { {loading} {error_view} {list} }
            };
            view! { <div class="task-history" id=history_panel_id.clone()>{body}</div> }
        })
    };
    let controls_id = history_id.clone();

    view! {
        <div class="task-card">
            <div class="task-card-header">
                <div class="task-name-wrap">
                    {header_dot}
                    <span class="task-name">
                        {move || task.with(|t| task_label(&t.name, t.display_name.as_deref()))}
                    </span>
                </div>
                <button
                    type="button"
                    class="run-btn"
                    disabled=move || task.with(|t| t.running)
                    on:click=move |_| on_run.run(name.get_untracked())
                >
                    {move || if task.with(|t| t.running) { "Running…" } else { "Run Now" }}
                </button>
            </div>
            <div class="task-schedule">{schedule}</div>
            <div class="task-next-run">
                <span class="field-label">"Next Run"</span>
                {next_run}
            </div>
            {last_run}
            {history_panel}
            <button
                type="button"
                class="history-toggle"
                aria-expanded=move || expanded.get().to_string()
                aria-controls=move || expanded.get().then(|| controls_id.clone())
                on:click=move |_| expanded.update(|v| *v = !*v)
            >
                {move || if expanded.get() { "Hide History" } else { "History" }}
                <span class=move || format!("chevron {}", if expanded.get() { "open" } else { "" })>
                    "▾"
                </span>
            </button>
        </div>
    }
}
