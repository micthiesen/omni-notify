//! Recent task runs with grouping and filters.

use leptos::prelude::*;
use omni_api::runs::{Run, RunStatus};
use omni_api::tasks::TaskInfo;

use super::badges::{StatusDot, TriggerBadge};
use super::task_card::run_duration_now;
use crate::api::{self, Snapshot};
use crate::task::spawn_scoped;
use crate::utils::format::{format_absolute, format_relative, task_label, task_label_from_name};

const FILTERED_LIMIT: u32 = 100;

/// Consecutive successful runs of one task collapse into one row (newest
/// first); errors and in-flight runs always get their own row.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityGroup {
    pub key: String,
    pub runs: Vec<Run>,
}

pub fn group_runs(runs: &[Run]) -> Vec<ActivityGroup> {
    let mut groups: Vec<ActivityGroup> = Vec::new();
    for run in runs {
        if let Some(last) = groups.last_mut()
            && run.status == RunStatus::Success
            && last.runs[0].status == RunStatus::Success
            && last.runs[0].task_name == run.task_name
        {
            last.runs.push(run.clone());
            continue;
        }
        groups.push(ActivityGroup {
            key: run.run_id.clone(),
            runs: vec![run.clone()],
        });
    }
    groups
}

#[component]
fn GroupRow(
    group: ActivityGroup,
    tasks: Vec<TaskInfo>,
    on_view_logs: Callback<Run>,
) -> impl IntoView {
    let newest = group.runs[0].clone();
    let oldest_started = group
        .runs
        .last()
        .map_or(newest.started_at, |r| r.started_at);
    let count = group.runs.len();
    let clicked = newest.clone();
    let detail = newest
        .error
        .clone()
        .map(|e| ("run-error", e))
        .or_else(|| newest.summary.clone().map(|s| ("run-summary", s)));
    view! {
        <button
            type="button"
            class="activity-row row-btn"
            on:click=move |_| on_view_logs.run(clicked.clone())
            title="View logs"
        >
            <StatusDot status=newest.status/>
            <span class="activity-task">{task_label_from_name(&newest.task_name, &tasks)}</span>
            {(count > 1)
                .then(|| {
                    view! {
                        <span
                            class="collapse-badge"
                            title=format!(
                                "{count} consecutive runs, oldest {}",
                                format_relative(oldest_started as f64),
                            )
                        >
                            {format!("×{count}")}
                        </span>
                    }
                })}
            <TriggerBadge trigger=newest.trigger/>
            <span class="activity-time" title=format_absolute(newest.started_at as f64)>
                {format_relative(newest.started_at as f64)}
            </span>
            <span class="activity-duration muted">{run_duration_now(&newest)}</span>
            {detail.map(|(class, text)| view! { <span class=format!("activity-detail {class}")>{text}</span> })}
        </button>
    }
}

#[component]
pub fn ActivityFeed(
    #[prop(into)] snapshot: Signal<Snapshot>,
    on_view_logs: Callback<Run>,
) -> impl IntoView {
    let filter_task = RwSignal::new(String::new());
    let errors_only = RwSignal::new(false);
    let fetched = RwSignal::new(None::<Vec<Run>>);
    let fetch_error = RwSignal::new(None::<String>);
    let filtered = Memo::new(move |_| !filter_task.get().is_empty() || errors_only.get());
    let newest_run_id =
        Memo::new(move |_| snapshot.with(|s| s.runs.first().map(|r| r.run_id.clone())));

    // Filtered views need deeper history than the snapshot carries; refetch
    // whenever new runs land so the view stays live.
    Effect::new(move |_| {
        if !filtered.get() {
            fetched.set(None);
            fetch_error.set(None);
            return;
        }
        let task = filter_task.get();
        newest_run_id.track();
        spawn_scoped(async move {
            let task = (!task.is_empty()).then_some(task);
            match api::fetch_task_runs(task.as_deref(), Some(FILTERED_LIMIT)).await {
                Ok(data) => {
                    fetched.set(Some(data.runs));
                    fetch_error.set(None);
                }
                Err(e) => fetch_error.set(Some(e.message().to_owned())),
            }
        });
    });

    let visible = Memo::new(move |_| {
        let errors = errors_only.get();
        let compute = |runs: &[Run]| {
            if errors {
                let only: Vec<Run> = runs
                    .iter()
                    .filter(|r| r.status == RunStatus::Error)
                    .cloned()
                    .collect();
                group_runs(&only)
            } else {
                group_runs(runs)
            }
        };
        if filtered.get() {
            fetched.with(|f| f.as_deref().map(compute))
        } else {
            Some(snapshot.with(|s| compute(&s.runs)))
        }
    });

    let list_state = Memo::new(move |_| visible.with(|v| v.as_ref().map(Vec::is_empty)));

    let options = move || {
        snapshot.with(|s| {
            s.tasks
                .iter()
                .map(|task| {
                    let name = task.name.clone();
                    let selected = {
                        let name = name.clone();
                        move || filter_task.get() == name
                    };
                    view! {
                        <option value=name selected=selected>
                            {task_label(&task.name, task.display_name.as_deref())}
                        </option>
                    }
                })
                .collect_view()
        })
    };

    view! {
        <div class="activity-controls">
            <select
                class="activity-filter"
                aria-label="Filter activity by task"
                prop:value=move || filter_task.get()
                on:change=move |ev| filter_task.set(event_target_value(&ev))
            >
                <option value="">"All Tasks"</option>
                {options}
            </select>
            <button
                type="button"
                class=move || format!("chip-btn {}", if errors_only.get() { "active" } else { "" })
                aria-pressed=move || errors_only.get().to_string()
                on:click=move |_| errors_only.update(|v| *v = !*v)
            >
                "Errors Only"
            </button>
        </div>
        {move || {
            (filtered.get() && fetched.with(Option::is_none))
                .then(|| fetch_error.get())
                .flatten()
                .map(|e| view! { <div class="error-inline">"Failed to load activity: " {e}</div> })
        }}
        {move || {
            (visible.with(Option::is_none) && fetch_error.with(Option::is_none))
                .then(|| view! { <div class="loading-inline">"Loading activity…"</div> })
        }}
        {move || {
            list_state
                .get()
                .map(|empty| {
                    if empty {
                        let text = if errors_only.get() {
                            "No errors recorded. 🎉"
                        } else {
                            "No task runs recorded yet."
                        };
                        view! { <div class="muted activity-empty">{text}</div> }.into_any()
                    } else {
                        view! {
                            <div class="activity-list">
                                <For
                                    each=move || visible.get().unwrap_or_default()
                                    key=|group| format!("{group:?}")
                                    children=move |group| {
                                        let tasks = snapshot.with_untracked(|s| s.tasks.clone());
                                        view! { <GroupRow group tasks on_view_logs/> }
                                    }
                                />
                            </div>
                        }
                            .into_any()
                    }
                })
        }}
    }
}

#[cfg(test)]
mod tests {
    use omni_api::runs::RunTrigger;

    use super::*;

    fn run(id: &str, task: &str, status: RunStatus) -> Run {
        Run {
            run_id: id.into(),
            task_name: task.into(),
            trigger: RunTrigger::Schedule,
            scheduled_for: None,
            started_at: 0,
            finished_at: None,
            status,
            error: None,
            summary: None,
        }
    }

    #[test]
    fn collapses_consecutive_successes_of_one_task() {
        let runs = [
            run("a", "LiveCheck", RunStatus::Success),
            run("b", "LiveCheck", RunStatus::Success),
            run("c", "LiveCheck", RunStatus::Error),
            run("d", "LiveCheck", RunStatus::Success),
            run("e", "Pets", RunStatus::Success),
            run("f", "Pets", RunStatus::Running),
        ];
        let groups = group_runs(&runs);
        let shape: Vec<(String, usize)> = groups
            .iter()
            .map(|g| (g.key.clone(), g.runs.len()))
            .collect();
        assert_eq!(
            shape,
            [
                ("a".into(), 2),
                ("c".into(), 1),
                ("d".into(), 1),
                ("e".into(), 1),
                ("f".into(), 1)
            ]
        );
    }
}
