//! Recent task runs as rows, with consecutive successes collapsed.

use leptos::prelude::*;
use omni_api::runs::{Run, RunStatus};
use omni_api::tasks::TaskInfo;

use super::badges::{Status, TriggerBadge, run_status_kind};
use super::controls::{SegOption, Segmented};
use super::states::{EmptyState, ErrorState, SkeletonRows};
use super::task_display::run_duration_now;
use super::tone::StatusKind;
use crate::api::{self, Snapshot};
use crate::task::spawn_scoped;
use crate::utils::format::{format_absolute, format_relative, task_label, task_label_from_name};

const FILTERED_LIMIT: u32 = 100;

/// Successful runs of one task collapse into one row (newest first) for as
/// long as no other outcome interrupts the stretch, so interleaved routine
/// tasks (live checks, event delivery) read as one row each. Errors,
/// degraded and in-flight runs always get their own row and end the stretch.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityGroup {
    pub key: String,
    pub runs: Vec<Run>,
}

pub fn group_runs(runs: &[Run]) -> Vec<ActivityGroup> {
    let mut groups: Vec<ActivityGroup> = Vec::new();
    // Index of the first group in the current all-success stretch.
    let mut stretch_start = 0;
    for run in runs {
        if run.status != RunStatus::Success {
            stretch_start = groups.len() + 1;
        } else if let Some(group) = groups
            .iter_mut()
            .skip(stretch_start)
            .find(|g| g.runs[0].task_name == run.task_name)
        {
            group.runs.push(run.clone());
            continue;
        }
        groups.push(ActivityGroup {
            key: run.run_id.clone(),
            runs: vec![run.clone()],
        });
    }
    groups
}

/// One run row: status, task, ×N, trigger, detail, time and duration.
#[component]
pub fn RunRow(
    group: ActivityGroup,
    #[prop(into)] label: String,
    on_select: Callback<Run>,
    #[prop(into, optional)] selected: Signal<bool>,
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
        .map(|e| (true, e))
        .or_else(|| newest.summary.clone().map(|s| (false, s)));
    let kind = run_status_kind(newest.status);
    let tone = match kind {
        StatusKind::Fault => " run-fault",
        StatusKind::Warn => " run-warn",
        StatusKind::Running => " run-running",
        _ => " run-ok",
    };
    view! {
        <button
            type="button"
            class=move || format!("row{tone}{}", if selected.get() { " selected" } else { "" })
            on:click=move |_| on_select.run(clicked.clone())
            title="View logs"
        >
            <Status kind dot_only=kind == StatusKind::Ok/>
            <span class="row-main">
                <span class="row-title">
                    {label}
                    {(count > 1).then(|| view! {
                        <span
                            class="count"
                            title=format!("{count} consecutive runs, oldest {}", format_relative(oldest_started as f64))
                        >
                            {format!("×{count}")}
                        </span>
                    })}
                </span>
                {detail.map(|(fault, text)| view! {
                    <span class=if fault { "row-sub text-fault truncate" } else { "row-sub truncate" }>{text}</span>
                })}
            </span>
            <span class="row-end">
                <TriggerBadge trigger=newest.trigger/>
                <span class="num dim" title=format_absolute(newest.started_at as f64)>
                    {format_relative(newest.started_at as f64)}
                </span>
                <span class="num off hide-phone">{run_duration_now(&newest)}</span>
            </span>
        </button>
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunFilter {
    All,
    Errors,
}

/// The run log: task filter, All/Errors and grouped rows. Filtered views
/// fetch deeper history and refetch when a new run lands.
#[component]
pub fn RunLog(
    #[prop(into)] snapshot: Signal<Snapshot>,
    on_select: Callback<Run>,
    #[prop(into, optional)] selected_run: Signal<Option<String>>,
    /// Restrict to one task (no task picker).
    #[prop(into, optional)]
    task: MaybeProp<String>,
) -> impl IntoView {
    let filter_task = RwSignal::new(task.get_untracked().unwrap_or_default());
    let fixed_task = task.get_untracked().is_some();
    let mode = RwSignal::new(RunFilter::All);
    let fetched = RwSignal::new(None::<Vec<Run>>);
    let fetch_error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(0u32);
    let filtered =
        Memo::new(move |_| !filter_task.get().is_empty() || mode.get() == RunFilter::Errors);
    let newest_run_id =
        Memo::new(move |_| snapshot.with(|s| s.runs.first().map(|r| r.run_id.clone())));

    Effect::new(move |_| {
        retry.track();
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
        let errors = mode.get() == RunFilter::Errors;
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

    let options = move || {
        snapshot.with(|s| {
            s.tasks
                .iter()
                .map(|task: &TaskInfo| {
                    let name = task.name.clone();
                    view! { <option value=name>{task_label(&task.name, task.display_name.as_deref())}</option> }
                })
                .collect_view()
        })
    };
    let modes = Signal::derive(|| {
        vec![
            SegOption::new(RunFilter::All, "All"),
            SegOption::new(RunFilter::Errors, "Errors"),
        ]
    });

    view! {
        <div class="toolbar">
            {(!fixed_task).then(|| view! {
                <select
                    class="select"
                    aria-label="Filter runs by task"
                    prop:value=move || filter_task.get()
                    on:change=move |ev| filter_task.set(event_target_value(&ev))
                >
                    <option value="">"All tasks"</option>
                    {options}
                </select>
            })}
            <Segmented
                options=modes
                value=mode
                on_change=Callback::new(move |m| mode.set(m))
                aria_label="Run status"
                small=true
            />
        </div>
        {move || {
            (filtered.get() && fetched.with(Option::is_none)).then(|| fetch_error.get()).flatten().map(|e| view! {
                <ErrorState
                    title="Could not load runs"
                    raw=e
                    retry=Callback::new(move |()| retry.update(|n| *n += 1))
                />
            })
        }}
        {move || match visible.get() {
            None => fetch_error.with(Option::is_none).then(|| view! { <SkeletonRows count=6/> }.into_any()),
            Some(groups) if groups.is_empty() => Some(view! {
                <EmptyState
                    compact=true
                    message=if mode.get() == RunFilter::Errors { "No failed runs." } else { "No runs recorded yet." }
                />
            }.into_any()),
            Some(groups) => Some(view! {
                <div class="rows">
                    {groups.into_iter().map(|group| {
                        let label = snapshot.with_untracked(|s| task_label_from_name(&group.runs[0].task_name, &s.tasks));
                        let ids: Vec<String> = group.runs.iter().map(|r| r.run_id.clone()).collect();
                        let selected = Signal::derive(move || selected_run.with(|s| s.as_ref().is_some_and(|id| ids.contains(id))));
                        view! { <RunRow group label on_select selected/> }
                    }).collect_view()}
                </div>
            }.into_any()),
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
    fn collapses_interleaved_successes_until_another_outcome() {
        let runs = [
            run("a", "LiveCheck", RunStatus::Success),
            run("b", "Events", RunStatus::Success),
            run("c", "LiveCheck", RunStatus::Success),
            run("d", "Events", RunStatus::Success),
            run("e", "Events", RunStatus::Degraded),
            run("f", "LiveCheck", RunStatus::Success),
        ];
        let shape: Vec<(String, usize)> = group_runs(&runs)
            .iter()
            .map(|g| (g.key.clone(), g.runs.len()))
            .collect();
        assert_eq!(
            shape,
            [
                ("a".into(), 2),
                ("b".into(), 2),
                ("e".into(), 1),
                ("f".into(), 1)
            ]
        );
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
