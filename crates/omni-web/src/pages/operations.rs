//! Task health, controls, run history and activity (`pages/OperationsPage.tsx`).

use leptos::prelude::*;
use omni_api::runs::{Run, RunStatus};
use omni_api::tasks::TaskInfo;
use omni_web_kit::components::{
    ActivityFeed, LogViewer, StatStrip, TaskCard, Toast, ToastKind, use_toast,
};
use omni_web_kit::task::spawn_detached;
use omni_web_kit::use_live_data;
use omni_web_kit::utils::format::task_label;
use omni_web_kit::utils::js::parse_date_ms;

use super::home::empty_snapshot;

fn next_run_ms(task: &TaskInfo) -> f64 {
    task.next_runs
        .first()
        .and_then(|iso| parse_date_ms(iso))
        .unwrap_or(f64::INFINITY)
}

fn rank(task: &TaskInfo) -> u8 {
    if task.running {
        0
    } else if task
        .last_run
        .as_ref()
        .is_some_and(|r| r.status == RunStatus::Error)
    {
        1
    } else {
        2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskFilter {
    All,
    Attention,
    Running,
}

/// Task names in display order for the query and filter.
pub fn sorted_task_names(
    tasks: &[TaskInfo],
    query: &str,
    filter_attention: Option<bool>,
) -> Vec<String> {
    let needle = query.trim().to_lowercase();
    let mut matching: Vec<&TaskInfo> = tasks
        .iter()
        .filter(|task| {
            let haystack = format!(
                "{} {}",
                task_label(&task.name, task.display_name.as_deref()),
                task.name
            )
            .to_lowercase();
            haystack.contains(&needle)
                && match filter_attention {
                    None => true,
                    Some(false) => task.running,
                    Some(true) => task
                        .last_run
                        .as_ref()
                        .is_some_and(|r| r.status == RunStatus::Error),
                }
        })
        .collect();
    matching.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| next_run_ms(a).total_cmp(&next_run_ms(b)))
    });
    matching.into_iter().map(|t| t.name.clone()).collect()
}

#[component]
pub fn OperationsPage() -> impl IntoView {
    let live = use_live_data();
    let has_snapshot = Memo::new(move |_| live.snapshot.with(Option::is_some));
    move || {
        if has_snapshot.get() {
            return view! { <OperationsContent/> }.into_any();
        }
        match live.error.get() {
            Some(error) => view! {
                <div class="error">
                    <div>"Failed to load operations"</div>
                    <div class="error-detail">{error}</div>
                </div>
            }
            .into_any(),
            None => view! { <div class="loading">"Loading…"</div> }.into_any(),
        }
    }
}

#[component]
fn OperationsContent() -> impl IntoView {
    let live = use_live_data();
    let toast = use_toast();
    let query = RwSignal::new(String::new());
    let task_filter = RwSignal::new(TaskFilter::All);
    let log_run = RwSignal::new(None::<Run>);
    let snapshot = Signal::derive(move || live.snapshot.get().unwrap_or_else(empty_snapshot));
    let task_count = Memo::new(move |_| snapshot.with(|s| s.tasks.len()));
    let sorted = Memo::new(move |_| {
        let filter = match task_filter.get() {
            TaskFilter::All => None,
            TaskFilter::Attention => Some(true),
            TaskFilter::Running => Some(false),
        };
        snapshot.with(|s| sorted_task_names(&s.tasks, &query.get(), filter))
    });

    let on_run = Callback::new(move |name: String| {
        spawn_detached(async move {
            let result = live.run_task(name, None).await;
            toast.show(
                result.message,
                if result.ok {
                    ToastKind::Info
                } else {
                    ToastKind::Error
                },
            );
        });
    });
    let on_view_logs = Callback::new(move |run: Run| log_run.set(Some(run)));
    let card = move |name: String| {
        let fallback =
            snapshot.with_untracked(|s| s.tasks.iter().find(|t| t.name == name).cloned());
        let task = Memo::new(move |previous: Option<&Option<TaskInfo>>| {
            snapshot
                .with(|s| s.tasks.iter().find(|t| t.name == name).cloned())
                .or_else(|| previous.cloned().flatten())
                .or_else(|| fallback.clone())
        });
        move || {
            task.get().is_some().then(|| {
                let task = Signal::derive(move || task.get().unwrap_or_else(placeholder_task));
                view! { <TaskCard task on_run on_view_logs/> }
            })
        }
    };
    let filters = [
        (TaskFilter::All, "All"),
        (TaskFilter::Attention, "Needs Attention"),
        (TaskFilter::Running, "Running"),
    ]
    .into_iter()
    .map(|(value, label)| {
        view! {
            <button
                type="button"
                class=move || format!("chip-btn {}", if task_filter.get() == value { "active" } else { "" })
                aria-pressed=move || (task_filter.get() == value).to_string()
                on:click=move |_| task_filter.set(value)
            >
                {label}
            </button>
        }
    })
    .collect_view();

    view! {
        <Toast toast=toast.toast/>
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"Operations"</h1>
                <p class="page-subtitle">"Task health, controls, run history, and system activity."</p>
            </div>
        </div>
        {move || {
            live.error.get().map(|error| view! {
                <div class="error-inline stale-note">
                    {format!("Refresh failed ({error}), showing last known state.")}
                </div>
            })
        }}
        <StatStrip snapshot/>
        <section class="page-section">
            <div class="section-heading-row">
                <h2 class="section-title">
                    "Tasks " <span class="section-count">{move || task_count.get()}</span>
                </h2>
            </div>
            <div class="task-toolbar">
                <input
                    class="task-search"
                    type="search"
                    aria-label="Search tasks"
                    placeholder="Search tasks…"
                    prop:value=move || query.get()
                    on:input=move |ev| query.set(event_target_value(&ev))
                />
                <div class="task-filters" aria-label="Task status">{filters}</div>
            </div>
            <Show
                when=move || sorted.with(|s| !s.is_empty())
                fallback=|| view! { <div class="muted">"No tasks match this view."</div> }
            >
                <div class="task-grid">
                    <For each=move || sorted.get() key=|name| name.clone() children=card/>
                </div>
            </Show>
        </section>
        <section class="page-section">
            <h2 class="section-title">"Activity"</h2>
            <ActivityFeed snapshot on_view_logs/>
        </section>
        {move || {
            log_run.get().map(|run| {
                view! { <LogViewer run on_close=Callback::new(move |()| log_run.set(None))/> }
            })
        }}
    }
}

fn placeholder_task() -> TaskInfo {
    TaskInfo {
        name: String::new(),
        display_name: None,
        schedule: String::new(),
        running: false,
        next_runs: Vec::new(),
        last_run: None,
    }
}
