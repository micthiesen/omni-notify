//! Recent runs of a recommendation task, as a disclosure of run rows.
//!
//! Shared by the recommendation pages. Refetches whenever `latest_run_id`
//! changes so a fresh run appears without a manual refresh. Each row opens
//! the run's logs.

use leptos::prelude::*;
use omni_api::runs::{Run, RunStatus};
use omni_web_kit::api;
use omni_web_kit::components::{
    Disclosure, EmptyState, InlineNote, LogViewer, SkeletonRows, Status, StatusKind, Tone,
};
use omni_web_kit::task::spawn_scoped;
use omni_web_kit::utils::format::{format_absolute, format_relative};

/// `(label, status shape)` of a run row. A successful run that added
/// nothing ("no_add: …") is a warning, not a failure.
pub fn run_outcome(run: &Run) -> (&'static str, StatusKind) {
    match run.status {
        RunStatus::Running => ("Running", StatusKind::Running),
        RunStatus::Error => ("Error", StatusKind::Fault),
        RunStatus::Degraded => ("Skipped", StatusKind::Warn),
        RunStatus::Success
            if run
                .summary
                .as_deref()
                .is_some_and(|s| s.starts_with("no_add:")) =>
        {
            ("No pick", StatusKind::Warn)
        }
        RunStatus::Success => ("Completed", StatusKind::Ok),
    }
}

#[component]
pub fn RecommendationRuns(
    task_name: &'static str,
    #[prop(into)] latest_run_id: Signal<Option<String>>,
) -> impl IntoView {
    let load_error = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let runs = RwSignal::new(Vec::<Run>::new());
    let log_run = RwSignal::new(None::<Run>);

    Effect::new(move |_| {
        latest_run_id.track();
        spawn_scoped(async move {
            match api::fetch_task_runs(Some(task_name), Some(6)).await {
                Ok(data) => {
                    runs.set(data.runs);
                    loaded.set(true);
                    load_error.set(false);
                }
                // Recommendation cards remain useful if activity history is unavailable.
                Err(_) => {
                    load_error.set(true);
                    loaded.set(true);
                }
            }
        });
    });

    let rows = move || {
        runs.get()
            .into_iter()
            .map(|run| {
                let (label, kind) = run_outcome(&run);
                let detail = run.error.clone().or_else(|| run.summary.clone());
                let started = run.started_at as f64;
                let target = run.clone();
                view! {
                    <button
                        type="button"
                        class="row rec-run"
                        title="View logs"
                        on:click=move |_| log_run.set(Some(target.clone()))
                    >
                        <Status kind=kind label=label dot_only=kind == StatusKind::Ok/>
                        <span class="row-main">
                            <span class="row-title">{label}</span>
                            {detail.map(|d| view! { <span class="row-sub">{d}</span> })}
                        </span>
                        <span class="row-end num small muted" title=format_absolute(started)>
                            {format_relative(started)}
                        </span>
                    </button>
                }
            })
            .collect_view()
    };

    let summary = Signal::derive(move || {
        let n = runs.with(Vec::len);
        if n == 0 {
            "Recent runs".to_owned()
        } else {
            format!("Recent runs · {n}")
        }
    });

    view! {
        <Disclosure summary class="rec-runs">
            {move || {
                load_error.get().then(|| view! {
                    <InlineNote tone=Tone::Warn>"Run history could not be refreshed."</InlineNote>
                })
            }}
            {move || {
                if !loaded.get() {
                    view! { <SkeletonRows count=3 label="Loading runs"/> }.into_any()
                } else if runs.with(Vec::is_empty) && !load_error.get() {
                    view! { <EmptyState message="No recommendation runs recorded yet." compact=true/> }
                        .into_any()
                } else {
                    view! { <div class="rows">{rows}</div> }.into_any()
                }
            }}
        </Disclosure>
        {move || {
            log_run
                .get()
                .map(|run| {
                    view! { <LogViewer run=run on_close=Callback::new(move |()| log_run.set(None))/> }
                })
        }}
    }
}

#[cfg(test)]
mod tests {
    use omni_api::runs::{Run, RunStatus, RunTrigger};
    use omni_web_kit::components::StatusKind;

    use super::run_outcome;

    fn run(status: RunStatus, summary: Option<&str>) -> Run {
        Run {
            run_id: "Recommendations:1".into(),
            task_name: "Recommendations".into(),
            trigger: RunTrigger::Manual,
            scheduled_for: None,
            started_at: 1,
            finished_at: None,
            status,
            error: None,
            summary: summary.map(str::to_owned),
        }
    }

    #[test]
    fn outcomes_follow_status_and_no_add_summaries() {
        assert_eq!(run_outcome(&run(RunStatus::Running, None)).0, "Running");
        assert_eq!(
            run_outcome(&run(RunStatus::Error, None)).1,
            StatusKind::Fault
        );
        assert_eq!(
            run_outcome(&run(RunStatus::Success, Some("no_add: dupes"))),
            ("No pick", StatusKind::Warn)
        );
        assert_eq!(
            run_outcome(&run(RunStatus::Success, Some("added 1"))),
            ("Completed", StatusKind::Ok)
        );
    }
}
