//! Recent runs of a recommendation task.
//!
//! Shared by the media and podcast pages. Refetches whenever `latest_run_id`
//! changes so a fresh run appears without a manual refresh.

use leptos::prelude::*;
use omni_api::runs::{Run, RunStatus};
use omni_web_kit::api;
use omni_web_kit::components::LogViewer;
use omni_web_kit::task::spawn_scoped;
use omni_web_kit::utils::format::{format_absolute, format_relative};

/// `(label, tone)` of a run row.
pub fn run_outcome(run: &Run) -> (&'static str, &'static str) {
    match run.status {
        RunStatus::Running => ("Running", "running"),
        RunStatus::Error => ("Error", "error"),
        RunStatus::Success
            if run
                .summary
                .as_deref()
                .is_some_and(|s| s.starts_with("no_add:")) =>
        {
            ("No Pick", "no-add")
        }
        RunStatus::Success => ("Completed", "success"),
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
                let (label, tone) = run_outcome(&run);
                let detail = run.error.clone().or_else(|| run.summary.clone());
                let detail_class = if run.error.is_some() {
                    "run-error"
                } else {
                    "run-summary"
                };
                let started = run.started_at as f64;
                let target = run.clone();
                view! {
                    <button
                        type="button"
                        class="rec-run-row row-btn"
                        title="View logs"
                        on:click=move |_| log_run.set(Some(target.clone()))
                    >
                        <span class=format!("rec-run-outcome rec-run-{tone}")>{label}</span>
                        <span class="rec-run-time" title=format_absolute(started)>
                            {format_relative(started)}
                        </span>
                        {detail.map(|detail| view! { <span class=detail_class>{detail}</span> })}
                    </button>
                }
            })
            .collect_view()
    };

    view! {
        <details class="page-section rec-activity-section content-disclosure">
            <summary>"Recent Activity"</summary>
            {move || {
                load_error
                    .get()
                    .then(|| view! { <div class="error-inline">"Activity could not be refreshed."</div> })
            }}
            {move || {
                if !loaded.get() {
                    view! { <div class="muted">"Loading activity…"</div> }.into_any()
                } else if runs.with(Vec::is_empty) && !load_error.get() {
                    view! { <div class="muted">"No recommendation runs recorded yet."</div> }
                        .into_any()
                } else {
                    view! { <div class="rec-run-list">{rows}</div> }.into_any()
                }
            }}
        </details>
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
        assert_eq!(run_outcome(&run(RunStatus::Error, None)).1, "error");
        assert_eq!(
            run_outcome(&run(RunStatus::Success, Some("no_add: dupes"))),
            ("No Pick", "no-add")
        );
        assert_eq!(
            run_outcome(&run(RunStatus::Success, Some("added 1"))),
            ("Completed", "success")
        );
    }
}
