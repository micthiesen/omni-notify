//! Home: attention panel, live streams, On Deck, research and system health
//! (`pages/HomePage.tsx`).

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::email::EmailActivityOutcome;
use omni_api::runs::RunStatus;
use omni_api::workspaces::{WorkspaceOverview, WorkspaceSubjectStatus};
use omni_web_kit::api;
use omni_web_kit::components::{LiveNow, OnDeck, StatStrip};
use omni_web_kit::router::Link;
use omni_web_kit::task::{on_cleanup_local, spawn_scoped};
use omni_web_kit::use_live_data;
use omni_web_kit::utils::format::format_relative;
use omni_web_kit::utils::js::now_ms;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[component]
pub fn HomePage() -> impl IntoView {
    let live = use_live_data();
    let workspaces = RwSignal::new(None::<Vec<WorkspaceOverview>>);
    let recent_email_problems = RwSignal::new(None::<usize>);
    let workspace_error = RwSignal::new(false);
    let email_error = RwSignal::new(false);
    let refresh = RwSignal::new(0u64);
    let latest_run_at = Memo::new(move |_| {
        live.snapshot.with(|s| {
            s.as_ref()
                .and_then(|s| s.runs.first()?.finished_at)
                .unwrap_or(0)
        })
    });

    let on_updated = Closure::<dyn FnMut()>::new(move || refresh.update(|n| *n += 1));
    let win = window();
    let _ = win
        .add_event_listener_with_callback("workspace-updated", on_updated.as_ref().unchecked_ref());
    on_cleanup_local(move || {
        let _ = win.remove_event_listener_with_callback(
            "workspace-updated",
            on_updated.as_ref().unchecked_ref(),
        );
    });

    Effect::new(move |_| {
        latest_run_at.track();
        refresh.track();
        spawn_scoped(async move {
            match api::fetch_workspaces().await {
                Ok(response) => {
                    workspaces.set(Some(response.workspaces));
                    workspace_error.set(false);
                }
                Err(_) => workspace_error.set(true),
            }
        });
        spawn_scoped(async move {
            match api::fetch_email_activity(None, Some(500)).await {
                Ok(response) => {
                    let since = now_ms() - 24.0 * 60.0 * 60.0 * 1000.0;
                    let problems = response
                        .activities
                        .iter()
                        .filter(|a| {
                            a.processed_at as f64 >= since
                                && matches!(
                                    a.outcome,
                                    EmailActivityOutcome::Partial
                                        | EmailActivityOutcome::Failed
                                        | EmailActivityOutcome::Error
                                )
                        })
                        .count();
                    recent_email_problems.set(Some(problems));
                    email_error.set(false);
                }
                Err(_) => email_error.set(true),
            }
        });
    });

    let active_subjects = Memo::new(move |_| {
        let mut subjects: Vec<(String, String, omni_api::workspaces::WorkspaceSubject)> =
            workspaces.with(|w| {
                w.iter()
                    .flatten()
                    .flat_map(|ws| {
                        ws.subjects
                            .iter()
                            .filter(|s| s.status == WorkspaceSubjectStatus::Active)
                            .map(|s| {
                                (
                                    ws.definition.id.clone(),
                                    ws.definition.title.clone(),
                                    s.clone(),
                                )
                            })
                    })
                    .collect()
            });
        subjects.sort_by_key(|s| std::cmp::Reverse(s.2.updated_at));
        subjects.truncate(4);
        subjects
    });

    let has_snapshot = Memo::new(move |_| live.snapshot.with(Option::is_some));
    move || {
        if has_snapshot.get() {
            return view! {
                <HomeContent workspaces recent_email_problems workspace_error email_error active_subjects/>
            }
            .into_any();
        }
        match live.error.get() {
            Some(error) => view! {
                <div class="error">
                    <div>"Failed to load home"</div>
                    <div class="error-detail">{error}</div>
                </div>
            }
            .into_any(),
            None => view! { <div class="loading">"Loading…"</div> }.into_any(),
        }
    }
}

#[component]
fn HomeContent(
    workspaces: RwSignal<Option<Vec<WorkspaceOverview>>>,
    recent_email_problems: RwSignal<Option<usize>>,
    workspace_error: RwSignal<bool>,
    email_error: RwSignal<bool>,
    active_subjects: Memo<Vec<(String, String, omni_api::workspaces::WorkspaceSubject)>>,
) -> impl IntoView {
    let live = use_live_data();
    let snapshot = Signal::derive(move || live.snapshot.get().unwrap_or_else(empty_snapshot));
    let failing = Memo::new(move |_| {
        snapshot.with(|s| {
            s.tasks
                .iter()
                .filter(|t| {
                    t.last_run
                        .as_ref()
                        .is_some_and(|r| r.status == RunStatus::Error)
                })
                .count()
        })
    });
    let pending_actions = Memo::new(move |_| {
        workspaces.with(|w| {
            w.iter()
                .flatten()
                .map(|ws| ws.pending_action_count as usize)
                .sum::<usize>()
        })
    });
    let open_papercuts = Memo::new(move |_| {
        workspaces.with(|w| {
            w.iter()
                .flatten()
                .map(|ws| ws.open_papercut_count as usize)
                .sum::<usize>()
        })
    });
    let attention_count = Memo::new(move |_| {
        failing.get()
            + pending_actions.get()
            + open_papercuts.get()
            + recent_email_problems.get().unwrap_or(0)
    });
    let unavailable = Memo::new(move |_| {
        workspace_error.get() || email_error.get() || live.error.with(Option::is_some)
    });
    let loading = Memo::new(move |_| {
        (workspaces.with(Option::is_none) && !workspace_error.get())
            || (recent_email_problems.get().is_none() && !email_error.get())
    });

    let panel_class = move || {
        let count = attention_count.get();
        format!(
            "attention-panel {} {}",
            if count == 0 {
                "attention-compact"
            } else {
                "attention-actionable"
            },
            if unavailable.get() {
                "attention-unknown"
            } else if loading.get() {
                "attention-loading"
            } else {
                ""
            }
        )
    };
    let heading = move || {
        if attention_count.get() > 0 {
            "Needs Attention"
        } else if unavailable.get() {
            "Status Unavailable"
        } else if loading.get() {
            "Checking In"
        } else {
            "All Clear"
        }
    };
    let detail = move || {
        let count = attention_count.get();
        if unavailable.get() {
            format!(
                "{count} known item{}; some status is unavailable.",
                plural(count)
            )
        } else if loading.get() {
            "Checking research and email activity…".to_owned()
        } else if count == 0 {
            "Everything is running cleanly.".to_owned()
        } else {
            format!("{count} item{} need a look.", plural(count))
        }
    };
    let total = move || {
        let count = attention_count.get();
        if unavailable.get() {
            "!".to_owned()
        } else if loading.get() {
            "…".to_owned()
        } else if count == 0 {
            "✓".to_owned()
        } else {
            count.to_string()
        }
    };
    let links = move || {
        (attention_count.get() > 0).then(|| {
            let pending = pending_actions.get();
            let failing = failing.get();
            let email = recent_email_problems.get().unwrap_or(0);
            let papercuts = open_papercuts.get();
            view! {
                <div class="attention-links">
                    {(pending > 0).then(|| view! {
                        <Link to="/workspaces" class="attention-item">
                            <strong>{pending}</strong>
                            <span>{format!("Research Approval{}", plural(pending))}</span>
                        </Link>
                    })}
                    {(failing > 0).then(|| view! {
                        <Link to="/operations" class="attention-item attention-danger">
                            <strong>{failing}</strong>
                            <span>{format!("Failed Task{}", plural(failing))}</span>
                        </Link>
                    })}
                    {(email > 0).then(|| view! {
                        <Link to="/emails" class="attention-item attention-danger">
                            <strong>{email}</strong>
                            <span>{format!("Email Issue{}", plural(email))}</span>
                        </Link>
                    })}
                    {(papercuts > 0).then(|| view! {
                        <Link to="/workspaces" class="attention-item">
                            <strong>{papercuts}</strong>
                            <span>{format!("Papercut{}", plural(papercuts))}</span>
                        </Link>
                    })}
                </div>
            }
        })
    };
    let unavailable_notes = move || {
        unavailable.get().then(|| {
            view! {
                <div class="attention-unavailable">
                    {move || workspace_error.get().then(|| view! { <span>"Research status could not be refreshed."</span> })}
                    {move || email_error.get().then(|| view! { <span>"Email status could not be refreshed."</span> })}
                </div>
            }
        })
    };
    let research = move || {
        let subjects = active_subjects.get();
        if !subjects.is_empty() {
            let cards = subjects
                .into_iter()
                .map(|(workspace_id, workspace_title, subject)| {
                    let to = format!(
                        "/workspaces/{}/{}",
                        encode_uri_component(&workspace_id),
                        encode_uri_component(&subject.subject_id)
                    );
                    let summary = if subject.summary.is_empty() {
                        "Research in progress.".to_owned()
                    } else {
                        subject.summary.clone()
                    };
                    view! {
                        <Link to=to class="home-research-card">
                            <span class="home-research-workspace">{workspace_title}</span>
                            <strong>{subject.title.clone()}</strong>
                            <p>{summary}</p>
                            <small>{format!("Updated {}", format_relative(subject.updated_at as f64))}</small>
                        </Link>
                    }
                })
                .collect_view();
            view! { <div class="home-research-grid">{cards}</div> }.into_any()
        } else if workspace_error.get() {
            view! {
                <p class="error-inline">
                    "Research could not be refreshed. "
                    <Link to="/workspaces">"Open Workspaces"</Link>
                </p>
            }
            .into_any()
        } else if workspaces.with(Option::is_none) {
            view! { <p class="loading-inline">"Loading your research…"</p> }.into_any()
        } else {
            view! {
                <Link to="/workspaces" class="home-research-empty">"Start an ongoing workspace ›"</Link>
            }
            .into_any()
        }
    };

    view! {
        <div class="page-header home-header">
            <div class="page-header-stack">
                <span class="home-eyebrow">"Overview"</span>
                <h1>"Home"</h1>
                <p class="page-subtitle">"Live streams, fresh picks, and ongoing research."</p>
            </div>
        </div>
        {move || {
            live.error.get().map(|error| view! {
                <div class="error-inline stale-note">
                    {format!("Refresh failed ({error}), showing last known state.")}
                </div>
            })
        }}
        <section class=panel_class aria-label="System attention">
            <div class="attention-heading">
                <div>
                    <span class="section-title">{heading}</span>
                    <p>{detail}</p>
                </div>
                <span class=move || {
                    format!(
                        "attention-total {}",
                        if attention_count.get() > 0 || unavailable.get() { "has-items" } else { "" },
                    )
                }>{total}</span>
            </div>
            {links}
            {unavailable_notes}
        </section>

        <LiveNow streamers=Signal::derive(move || snapshot.with(|s| s.streamers.clone()))/>
        <OnDeck items=Signal::derive(move || snapshot.with(|s| s.on_deck.clone()))/>

        <section class="page-section home-research">
            <div class="section-heading-row">
                <h2 class="section-title">"Research"</h2>
                <Link to="/workspaces" class="section-view-all">"View Workspaces ›"</Link>
            </div>
            {research}
        </section>

        <section class="page-section home-system-health">
            <div class="section-heading-row">
                <h2 class="section-title">"System Health"</h2>
                <Link to="/operations" class="section-view-all">"Open Operations ›"</Link>
            </div>
            <StatStrip snapshot/>
        </section>
    }
}

pub(crate) fn empty_snapshot() -> api::Snapshot {
    api::Snapshot {
        tasks: Vec::new(),
        streamers: Vec::new(),
        runs: Vec::new(),
        on_deck: Vec::new(),
    }
}
