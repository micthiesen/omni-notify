//! Research workspaces: the overview, one workspace, and one subject's
//! dossier (outline plus one section at a time).

use std::collections::HashMap;
use std::time::Duration;

use futures::future::{Either, select};
use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::runs::RunStatus;
use omni_api::workspaces::{
    WorkspaceAction, WorkspaceActionStatus, WorkspaceDefinition, WorkspaceMessageRole,
    WorkspaceOverview, WorkspaceResponse, WorkspaceSourceKind, WorkspaceSubject,
    WorkspaceSubjectResponse, WorkspaceSubjectStatus,
};
use omni_web_kit::api::{self, ActionResolution, ApiClientError};
use omni_web_kit::chrome::use_page_label;
use omni_web_kit::components::{
    Button, ButtonSize, ButtonVariant, ConfirmButton, Disclosure, EmptyState, ErrorState, Icon,
    PageHead, Panel, Readout, ReadoutBand, SegOption, Segmented, SkeletonRows, Status, StatusKind,
    Tag, Tone,
};
use omni_web_kit::feeds::use_workspace_feed;
use omni_web_kit::hooks::{query_param, replace_query_param, scroll_into_view_center, use_now};
use omni_web_kit::live::use_live_data;
use omni_web_kit::markdown::WorkspaceMarkdown;
use omni_web_kit::router::Link;
use omni_web_kit::task::{sleep, spawn_detached, spawn_scoped};
use omni_web_kit::utils::cron::describe_cron;
use omni_web_kit::utils::format::{format_absolute, format_relative_at};
use omni_web_kit::utils::js::number_string;
use omni_web_kit::utils::tasks::{format_next, next_run_ms};
use serde_json::Value;

use crate::research_nav::{ResearchSwitch, ResearchTab};

const RUN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Subjects listed per workspace on the overview before "All N".
const OVERVIEW_SUBJECTS: usize = 5;

/// Workspace pages and the nav listen for this to refresh their counts.
fn notify_workspace_updated() {
    if let Ok(event) = web_sys::Event::new("workspace-updated") {
        let _ = window().dispatch_event(&event);
    }
}

/// `JSON.stringify(value, null, 2)` (JS number formatting, insertion order).
pub fn js_stringify_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(value, 0, &mut out);
    out
}

fn write_pretty(value: &Value, depth: usize, out: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() => out.push_str(&number_string(f)),
            _ => out.push_str("null"),
        },
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                write_pretty(item, depth + 1, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(depth));
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push_str("{\n");
            for (i, (key, item)) in map.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                out.push_str(&serde_json::to_string(key).unwrap_or_default());
                out.push_str(": ");
                write_pretty(item, depth + 1, out);
                if i + 1 < map.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(depth));
            out.push('}');
        }
    }
}

/// The stored JSON, pretty-printed.
pub fn format_action_payload(payload: &str) -> String {
    match serde_json::from_str::<Value>(payload) {
        Ok(value) => js_stringify_pretty(&value),
        Err(_) => "Stored action details are unavailable.".to_owned(),
    }
}

/// Only http(s) URLs, normalized by the URL parser.
fn safe_source_href(value: Option<&str>) -> Option<String> {
    let value = value.filter(|v| !v.is_empty())?;
    let url = web_sys::Url::new(value).ok()?;
    let protocol = url.protocol();
    (protocol == "http:" || protocol == "https:").then(|| url.href())
}

fn source_kind_str(kind: WorkspaceSourceKind) -> &'static str {
    match kind {
        WorkspaceSourceKind::Web => "web",
        WorkspaceSourceKind::Email => "email",
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// A subject's status: active is the quiet default, the rest say so.
fn subject_status(status: WorkspaceSubjectStatus) -> (StatusKind, &'static str) {
    match status {
        WorkspaceSubjectStatus::Active => (StatusKind::Ok, "Active"),
        WorkspaceSubjectStatus::Paused => (StatusKind::Idle, "Paused"),
        WorkspaceSubjectStatus::Completed => (StatusKind::Idle, "Completed"),
        WorkspaceSubjectStatus::Archived => (StatusKind::Idle, "Archived"),
    }
}

fn action_status(status: WorkspaceActionStatus) -> (StatusKind, &'static str) {
    match status {
        WorkspaceActionStatus::Pending => (StatusKind::Warn, "Waiting on you"),
        WorkspaceActionStatus::Approved => (StatusKind::Ok, "Approved"),
        WorkspaceActionStatus::Rejected => (StatusKind::Idle, "Rejected"),
        WorkspaceActionStatus::Failed => (StatusKind::Fault, "Failed"),
    }
}

fn subject_href(workspace_id: &str, subject_id: &str) -> String {
    format!(
        "/workspaces/{}/{}",
        encode_uri_component(workspace_id),
        encode_uri_component(subject_id)
    )
}

/// `Every day at 09:00` / `On demand` for a workspace's research sweep.
fn schedule_label(def: &WorkspaceDefinition) -> String {
    if def.scheduled_runs == Some(false) {
        return "On demand".to_owned();
    }
    describe_cron(&def.schedule).unwrap_or_else(|| def.schedule.clone())
}

/// The workspace task's next fire from the live snapshot.
fn use_next_sweep(task_name: Signal<Option<String>>) -> Memo<Option<f64>> {
    let live = use_live_data();
    Memo::new(move |_| {
        let name = task_name.get()?;
        live.snapshot.with(|s| {
            s.as_ref()?
                .tasks
                .iter()
                .find(|t| t.name == name)
                .and_then(next_run_ms)
        })
    })
}

/// Polls the run until it settles: success, its error, or a timeout.
async fn wait_for_run(run_id: &str) -> Result<(), String> {
    let poll = async {
        loop {
            match api::fetch_run_logs(run_id).await {
                Ok(logs) => match logs.run.status {
                    RunStatus::Success => return Ok(()),
                    RunStatus::Error => {
                        return Err(logs
                            .run
                            .error
                            .unwrap_or_else(|| "Workspace run failed".to_owned()));
                    }
                    RunStatus::Degraded => {
                        return Err(logs
                            .run
                            .error
                            .unwrap_or_else(|| "Workspace run skipped its work".to_owned()));
                    }
                    RunStatus::Running => sleep(Duration::from_secs(1)).await,
                },
                // The run row may not exist yet right after it was accepted.
                Err(ApiClientError::Api { status: 404, .. }) => {
                    sleep(Duration::from_millis(250)).await;
                }
                Err(err) => return Err(err.message().to_owned()),
            }
        }
    };
    let timeout = sleep(RUN_TIMEOUT);
    match select(Box::pin(poll), Box::pin(timeout)).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Err(
            "Workspace research is still running. Refresh shortly to see the result.".to_owned(),
        ),
    }
}

/// Stays mounted while the workspace or subject changes (both are signals).
#[component]
pub fn WorkspacesPage(
    #[prop(into)] workspace_id: Signal<Option<String>>,
    #[prop(into)] subject_id: Signal<Option<String>>,
) -> impl IntoView {
    let mode = Memo::new(move |_| {
        match (
            workspace_id.with(Option::is_some),
            subject_id.with(Option::is_some),
        ) {
            (true, true) => 2,
            (true, false) => 1,
            _ => 0,
        }
    });
    move || match mode.get() {
        2 => {
            let workspace = Signal::derive(move || workspace_id.get().unwrap_or_default());
            let subject = Signal::derive(move || subject_id.get().unwrap_or_default());
            view! { <SubjectPage workspace_id=workspace subject_id=subject /> }.into_any()
        }
        1 => {
            let workspace = Signal::derive(move || workspace_id.get().unwrap_or_default());
            view! { <WorkspacePage workspace_id=workspace /> }.into_any()
        }
        _ => view! { <Overview /> }.into_any(),
    }
}

// ===== Compose =====

/// "Start research" form: sends a message (optionally to a subject), waits
/// for the run, clears the text and calls `on_done`.
#[component]
fn Compose(
    #[prop(into)] workspace_id: Signal<String>,
    #[prop(into, optional)] subject_id: Signal<Option<String>>,
    #[prop(into)] placeholder: Signal<String>,
    #[prop(into)] label: String,
    #[prop(into)] send_label: String,
    #[prop(optional)] primary: bool,
    #[prop(optional)] compact: bool,
    busy: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_done: Callback<()>,
) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let send = move || {
        let message = text.get_untracked().trim().to_owned();
        if message.is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        let workspace = workspace_id.get_untracked();
        let subject = subject_id.get_untracked();
        spawn_detached(async move {
            let result: Result<(), String> = async {
                let accepted =
                    api::send_workspace_message(&workspace, &message, subject.as_deref())
                        .await
                        .map_err(|e| e.message().to_owned())?;
                wait_for_run(&accepted.run_id).await?;
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    text.try_set(String::new());
                    notify_workspace_updated();
                    on_done.run(());
                }
                Err(message) => {
                    error.try_set(Some(message));
                }
            }
            busy.try_set(false);
        });
    };
    let variant = if primary {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Secondary
    };
    let size = if compact {
        ButtonSize::Sm
    } else {
        ButtonSize::Md
    };
    view! {
        <form
            class=if compact { "ws-compose compact" } else { "ws-compose" }
            on:submit=move |event: leptos::ev::SubmitEvent| {
                event.prevent_default();
                send();
            }
        >
            <textarea
                class="textarea"
                rows=if compact { "1" } else { "3" }
                prop:value=move || text.get()
                on:input=move |event| text.set(event_target_value(&event))
                on:keydown=move |event: web_sys::KeyboardEvent| {
                    if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
                        event.prevent_default();
                        send();
                    }
                }
                placeholder=move || placeholder.get()
                aria-label=label
            ></textarea>
            <div class="ws-compose-foot">
                {move || {
                    busy.get()
                        .then(|| {
                            view! {
                                <Status
                                    kind=StatusKind::Running
                                    label="Researching, this can take a few minutes"
                                />
                            }
                        })
                }}
                <span class="spacer"></span>
                <Button
                    variant
                    size
                    submit=true
                    busy
                    disabled=Signal::derive(move || text.with(|t| t.trim().is_empty()))
                    disabled_reason="Write a message first"
                >
                    {send_label}
                </Button>
            </div>
        </form>
    }
}

// ===== Overview =====

#[component]
fn SubjectRow(
    workspace_id: String,
    subject: WorkspaceSubject,
    now: ReadSignal<f64>,
) -> impl IntoView {
    let (kind, word) = subject_status(subject.status);
    let summary = if subject.summary.is_empty() {
        "Work just started.".to_owned()
    } else {
        subject.summary.clone()
    };
    let updated = subject.updated_at as f64;
    let quiet = subject.status != WorkspaceSubjectStatus::Active;
    view! {
        <Link to=subject_href(&workspace_id, &subject.subject_id) class=if quiet { "row ws-subject-row quiet" } else { "row ws-subject-row" }>
            <Status kind=kind label=word dot_only=kind == StatusKind::Ok />
            <span class="row-main">
                <span class="row-title">{subject.title.clone()}</span>
                <span class="row-sub">{summary}</span>
            </span>
            <span class="row-end mono small muted" title=format_absolute(updated)>
                {move || format_relative_at(updated, now.get())}
            </span>
        </Link>
    }
}

#[component]
fn Overview() -> impl IntoView {
    let feed = use_workspace_feed();
    let now = use_now(30_000);
    let error = RwSignal::new(None::<String>);
    let tick = use_now(1000);

    let totals = Memo::new(move |_| {
        feed.workspaces.with(|w| {
            w.as_ref().map(|w| {
                (
                    w.iter().map(|o| o.active_subject_count).sum::<u64>() as usize,
                    w.iter().map(|o| o.pending_action_count).sum::<u64>() as usize,
                )
            })
        })
    });
    let sentence = Signal::derive(move || match totals.get() {
        None => "Research".to_owned(),
        Some((active, 0)) => format!(
            "{} in progress, nothing waiting on you.",
            plural(active, "subject", "subjects")
        ),
        Some((active, pending)) => format!(
            "{} in progress, {} waiting on you.",
            plural(active, "subject", "subjects"),
            plural(pending, "action", "actions")
        ),
    });

    let card = move |overview: WorkspaceOverview| {
        let def = overview.definition.clone();
        let id = def.id.clone();
        let label = def.subject_label.to_lowercase();
        let total = overview.subjects.len();
        let mut subjects: Vec<WorkspaceSubject> = overview
            .subjects
            .iter()
            .filter(|s| s.status != WorkspaceSubjectStatus::Archived)
            .cloned()
            .collect();
        subjects.sort_by_key(|s| s.status != WorkspaceSubjectStatus::Active);
        subjects.truncate(OVERVIEW_SUBJECTS);
        let shown = subjects.len();
        let rows = if subjects.is_empty() {
            view! {
                <p class="ws-empty small muted">{format!("Message the workspace to start the first {label}.")}</p>
            }
            .into_any()
        } else {
            subjects
                .into_iter()
                .map(|subject| view! { <SubjectRow workspace_id=id.clone() subject now /> })
                .collect_view()
                .into_any()
        };
        let task = def.task_name.clone();
        let next = use_next_sweep(Signal::stored(Some(task)));
        let scheduled = def.scheduled_runs != Some(false);
        let schedule = schedule_label(&def);
        let placeholder = def
            .input_placeholder
            .clone()
            .unwrap_or_else(|| format!("What would you like help with for this {label}?"));
        let busy = RwSignal::new(false);
        let pending = overview.pending_action_count as usize;
        let papercuts = overview.open_papercut_count as usize;
        let href = format!("/workspaces/{}", encode_uri_component(&id));
        let all_href = href.clone();
        view! {
            <Panel class="ws-card">
                <div class="ws-card-head">
                    <div class="ws-card-id">
                        <Link to=href class="ws-card-title">{def.title.clone()}</Link>
                        <p class="small muted clamp-2">{def.description.clone()}</p>
                    </div>
                    <div class="ws-card-meta">
                        {(pending > 0)
                            .then(|| view! { <Tag tone=Tone::Warn>{plural(pending, "action waiting", "actions waiting")}</Tag> })}
                        {(papercuts > 0).then(|| view! { <Tag>{plural(papercuts, "papercut", "papercuts")}</Tag> })}
                        <span class="mono small muted" title=def.schedule.clone()>
                            {schedule}
                            {move || {
                                scheduled
                                    .then(|| next.get())
                                    .flatten()
                                    .map(|at| format!(" · next {}", format_next(at - tick.get())))
                            }}
                        </span>
                    </div>
                </div>
                <div class="rows">{rows}</div>
                {(total > shown)
                    .then(|| {
                        view! {
                            <Link to=all_href class="row ws-more small">
                                {format!("All {total} {}", def.subject_label_plural.to_lowercase())}
                            </Link>
                        }
                    })}
                <div class="ws-card-foot">
                    <Compose
                        workspace_id=overview.definition.id.clone()
                        placeholder=placeholder
                        label=format!("Message {}", overview.definition.title)
                        send_label="Start"
                        compact=true
                        busy
                        error
                        on_done=Callback::new(move |()| feed.refresh())
                    />
                </div>
            </Panel>
        }
    };

    let phase = Memo::new(move |_| {
        feed.workspaces.with(|w| match w {
            None => Phase::Loading,
            Some(w) if w.is_empty() => Phase::Empty,
            Some(_) => Phase::Ready,
        })
    });

    view! {
        <PageHead title=sentence sentence=true lede="Research, decisions and next steps for ongoing projects." />
        <ResearchSwitch current=ResearchTab::Workspaces />
        {move || {
            error.get().map(|e| view! { <ErrorState title="That message could not start research" raw=e /> })
        }}
        {move || match phase.get() {
            Phase::Loading => match feed.error.get() {
                Some(e) => view! {
                    <ErrorState
                        title="Workspaces could not load"
                        raw=e
                        retry=Callback::new(move |()| feed.refresh())
                        page=true
                    />
                }
                .into_any(),
                None => view! { <SkeletonRows count=6 label="Loading workspaces" /> }.into_any(),
            },
            Phase::Empty => {
                view! { <EmptyState message="No workspaces are configured." icon=Icon::Flask /> }.into_any()
            }
            Phase::Ready => view! {
                <div class="ws-cards">
                    <For
                        each=move || feed.workspaces.get().unwrap_or_default()
                        key=|w| format!("{}|{w:?}", w.definition.id)
                        children=card
                    />
                </div>
            }
            .into_any(),
        }}
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Loading,
    Empty,
    Ready,
}

// ===== One workspace =====

#[component]
fn WorkspacePage(#[prop(into)] workspace_id: Signal<String>) -> impl IntoView {
    let detail = RwSignal::new(None::<WorkspaceResponse>);
    let error = RwSignal::new(None::<String>);
    let send_error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let reload = RwSignal::new(0u32);
    let now = use_now(30_000);
    let tick = use_now(1000);

    Effect::new(move |_| {
        let id = workspace_id.get();
        reload.track();
        spawn_scoped(async move {
            match api::fetch_workspace(&id).await {
                Ok(response) => {
                    detail.set(Some(response));
                    error.set(None);
                }
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    });
    use_page_label(move || detail.with(|d| d.as_ref().map(|d| d.workspace.title.clone())));

    let task =
        Signal::derive(move || detail.with(|d| d.as_ref().map(|d| d.workspace.task_name.clone())));
    let next = use_next_sweep(task);
    let pending_by_subject = Memo::new(move |_| {
        let mut counts = HashMap::<String, usize>::new();
        detail.with(|d| {
            for action in d.iter().flat_map(|d| d.actions.iter()) {
                if action.status == WorkspaceActionStatus::Pending {
                    *counts.entry(action.subject_id.clone()).or_default() += 1;
                }
            }
        });
        counts
    });

    let body = move || {
        let d = detail.get()?;
        let def = d.workspace.clone();
        let active = d
            .subjects
            .iter()
            .filter(|s| s.status == WorkspaceSubjectStatus::Active)
            .count();
        let pending: usize = pending_by_subject.with(|p| p.values().sum());
        let papercuts = d.papercuts.len();
        let label = def.subject_label.to_lowercase();
        let placeholder = def
            .input_placeholder
            .clone()
            .unwrap_or_else(|| format!("What would you like help with for this {label}?"));
        let scheduled = def.scheduled_runs != Some(false);
        let schedule = schedule_label(&def);
        let groups = [
            WorkspaceSubjectStatus::Active,
            WorkspaceSubjectStatus::Paused,
            WorkspaceSubjectStatus::Completed,
            WorkspaceSubjectStatus::Archived,
        ]
        .into_iter()
        .filter_map(|status| {
            let subjects: Vec<WorkspaceSubject> = d
                .subjects
                .iter()
                .filter(|s| s.status == status)
                .cloned()
                .collect();
            (!subjects.is_empty()).then_some((status, subjects))
        })
        .map(|(status, subjects)| {
            let (_, word) = subject_status(status);
            let count = subjects.len();
            let workspace = def.id.clone();
            let rows = subjects
                .into_iter()
                .map(|subject| {
                    let waiting = pending_by_subject.with(|p| p.get(&subject.subject_id).copied().unwrap_or(0));
                    let (kind, word) = subject_status(subject.status);
                    let updated = subject.updated_at as f64;
                    let summary = if subject.summary.is_empty() { "Work just started.".to_owned() } else { subject.summary.clone() };
                    view! {
                        <Link to=subject_href(&workspace, &subject.subject_id) class="row ws-subject-row">
                            <Status kind=kind label=word dot_only=kind == StatusKind::Ok />
                            <span class="row-main">
                                <span class="row-title">{subject.title.clone()}</span>
                                <span class="row-sub">{summary}</span>
                            </span>
                            <span class="row-end">
                                {(waiting > 0).then(|| view! { <Tag tone=Tone::Warn>{plural(waiting, "waiting", "waiting")}</Tag> })}
                                <span class="mono small muted hide-phone" title=format_absolute(updated)>
                                    {move || format_relative_at(updated, now.get())}
                                </span>
                            </span>
                        </Link>
                    }
                })
                .collect_view();
            view! {
                <div class="group-head">
                    {word}
                    <span class="seg-n">{count}</span>
                </div>
                <div class="rows">{rows}</div>
            }
        })
        .collect_view();
        let empty = d.subjects.is_empty().then(|| {
            view! { <EmptyState compact=true message=format!("No {} yet. Describe one above to start.", def.subject_label_plural.to_lowercase()) /> }
        });
        let pending_tone = if pending > 0 {
            Tone::Warn
        } else {
            Tone::Neutral
        };
        Some(view! {
            <PageHead title=def.title.clone() lede=def.description.clone() />
            <ReadoutBand cols=4 aria_label="Workspace">
                <Readout label="Active" value=active.to_string() />
                <Readout
                    label="Waiting on you"
                    value=pending.to_string()
                    tone=pending_tone
                />
                <Readout label="Papercuts" value=papercuts.to_string() />
                <Readout
                    label="Next sweep"
                    value=Signal::derive(move || {
                        if !scheduled {
                            return "—".to_owned();
                        }
                        next.get().map_or_else(|| "—".to_owned(), |at| format_next(at - tick.get()))
                    })
                >
                    <span class="readout-sub" title=def.schedule.clone()>{schedule}</span>
                </Readout>
            </ReadoutBand>
            <Panel title=format!("New {label}") pad=true class="ws-new">
                <Compose
                    workspace_id=def.id.clone()
                    placeholder=placeholder
                    label=format!("Message {}", def.title)
                    send_label="Start research"
                    primary=true
                    busy
                    error=send_error
                    on_done=Callback::new(move |()| reload.update(|n| *n += 1))
                />
                {move || send_error.get().map(|e| view! { <ErrorState title="That message could not start research" raw=e /> })}
            </Panel>
            <Panel
                title=def.subject_label_plural.clone()
                head_end=ViewFn::from(move || view! { <span class="num">{d.subjects.len()}</span> })
                class="ws-subjects"
            >
                {groups}
                {empty}
            </Panel>
        })
    };

    let loaded = Memo::new(move |_| detail.with(Option::is_some));
    move || {
        if !loaded.get() {
            return match error.get() {
                Some(e) => view! {
                    <ErrorState
                        title="This workspace could not load"
                        raw=e
                        retry=Callback::new(move |()| reload.update(|n| *n += 1))
                        link=("All workspaces".to_owned(), "/workspaces".to_owned())
                        page=true
                    />
                }
                .into_any(),
                None => view! { <SkeletonRows count=6 label="Loading workspace" /> }.into_any(),
            };
        }
        view! {
            {move || error.get().map(|e| view! { <ErrorState title="Refresh failed, showing the last loaded state" raw=e warn=true /> })}
            {body}
        }
        .into_any()
    }
}

// ===== One subject =====

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectSection {
    Actions,
    Artifacts,
    Conversation,
    Sources,
    Papercuts,
}

impl SubjectSection {
    const ALL: [SubjectSection; 5] = [
        Self::Actions,
        Self::Artifacts,
        Self::Conversation,
        Self::Sources,
        Self::Papercuts,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Actions => "actions",
            Self::Artifacts => "artifacts",
            Self::Conversation => "conversation",
            Self::Sources => "sources",
            Self::Papercuts => "papercuts",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Actions => "Actions",
            Self::Artifacts => "Artifacts",
            Self::Conversation => "Conversation",
            Self::Sources => "Sources",
            Self::Papercuts => "Papercuts",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }
}

/// The section and artifact tab a `?section=&target=` link selects. A target
/// picks its own section unless `section` names one.
pub fn deep_link_selection(
    section: Option<&str>,
    target: Option<&str>,
) -> (Option<SubjectSection>, Option<String>) {
    let artifact = target
        .and_then(|t| t.strip_prefix("artifact-"))
        .filter(|k| !k.is_empty())
        .map(str::to_owned);
    let from_target = match target {
        Some(t) if t.starts_with("action-") => Some(SubjectSection::Actions),
        Some(_) if artifact.is_some() => Some(SubjectSection::Artifacts),
        _ => None,
    };
    (
        section.and_then(SubjectSection::parse).or(from_target),
        artifact,
    )
}

/// Opening section without a link: pending actions first, else artifacts.
pub fn default_section(actions: &[WorkspaceAction]) -> SubjectSection {
    if actions
        .iter()
        .any(|a| a.status == WorkspaceActionStatus::Pending)
    {
        SubjectSection::Actions
    } else {
        SubjectSection::Artifacts
    }
}

#[component]
fn SubjectPage(
    #[prop(into)] workspace_id: Signal<String>,
    #[prop(into)] subject_id: Signal<String>,
) -> impl IntoView {
    let detail = RwSignal::new(None::<WorkspaceSubjectResponse>);
    let busy = RwSignal::new(false);
    let sending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let chosen = RwSignal::new(None::<SubjectSection>);
    let artifact_tab = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);
    let linked = StoredValue::new(None::<String>);
    let now = use_now(30_000);

    Effect::new(move |_| {
        let (w, s) = (workspace_id.get(), subject_id.get());
        // A new subject reads its deep link afresh.
        let (section, artifact) = deep_link_selection(
            query_param("section").as_deref(),
            query_param("target").as_deref(),
        );
        chosen.set(section);
        artifact_tab.set(artifact);
        detail.set(None);
        error.set(None);
        spawn_scoped(async move {
            match api::fetch_workspace_subject(&w, &s).await {
                Ok(response) => detail.set(Some(response)),
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    });
    Effect::new(move |previous: Option<u32>| {
        let n = reload.get();
        if previous.is_some() {
            let (w, s) = (workspace_id.get_untracked(), subject_id.get_untracked());
            spawn_scoped(async move {
                match api::fetch_workspace_subject(&w, &s).await {
                    Ok(response) => detail.set(Some(response)),
                    Err(err) => error.set(Some(err.message().to_owned())),
                }
            });
        }
        n
    });
    use_page_label(move || detail.with(|d| d.as_ref().map(|d| d.subject.title.clone())));

    let section = Memo::new(move |_| {
        chosen.get().unwrap_or_else(|| {
            detail.with(|d| {
                d.as_ref()
                    .map_or(SubjectSection::Artifacts, |d| default_section(&d.actions))
            })
        })
    });
    let artifact_keys = Memo::new(move |_| {
        detail.with(|d| {
            d.as_ref()
                .map(|d| {
                    d.workspace
                        .artifacts
                        .iter()
                        .map(|a| (a.key.clone(), a.title.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let current_artifact = Memo::new(move |_| {
        let keys = artifact_keys.get();
        artifact_tab
            .get()
            .filter(|k| keys.iter().any(|(key, _)| key == k))
            .or_else(|| keys.first().map(|(k, _)| k.clone()))
    });

    let select_section = move |next: SubjectSection| {
        chosen.set(Some(next));
        replace_query_param("section", Some(next.as_str()));
        replace_query_param("target", None);
    };
    let select_artifact = move |key: String| {
        artifact_tab.set(Some(key));
        select_section(SubjectSection::Artifacts);
    };

    // `?target=` deep links (Pushover) scroll to and highlight an element,
    // once per subject, after its section is visible.
    Effect::new(move |_| {
        if detail.with(Option::is_none) {
            return;
        }
        let subject = subject_id.get_untracked();
        if linked.get_value().as_deref() == Some(subject.as_str()) {
            return;
        }
        linked.set_value(Some(subject));
        request_animation_frame(move || {
            let Some(target) = query_param("target").filter(|t| !t.is_empty()) else {
                return;
            };
            let Some(element) = document().get_element_by_id(&target) else {
                return;
            };
            let _ = element.class_list().add_1("deep-link-target");
            scroll_into_view_center(&target);
        });
    });

    let counts = Memo::new(move |_| {
        detail.with(|d| {
            d.as_ref().map(|d| {
                let pending = d
                    .actions
                    .iter()
                    .filter(|a| a.status == WorkspaceActionStatus::Pending)
                    .count();
                (
                    pending,
                    d.actions.len(),
                    d.artifacts.len(),
                    d.messages.len(),
                    d.sources.len(),
                    d.papercuts.len(),
                )
            })
        })
    });
    let count_of = move |s: SubjectSection| {
        counts.get().map_or(
            0,
            |(_, actions, artifacts, messages, sources, papercuts)| match s {
                SubjectSection::Actions => actions,
                SubjectSection::Artifacts => artifacts,
                SubjectSection::Conversation => messages,
                SubjectSection::Sources => sources,
                SubjectSection::Papercuts => papercuts,
            },
        )
    };
    let pending_count = move || counts.get().map_or(0, |c| c.0);

    // Runs `action`, then reloads and notifies; errors land in `error`.
    let after =
        move |action: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>| {
            busy.set(true);
            error.set(None);
            spawn_detached(async move {
                match action.await {
                    Ok(()) => {
                        reload.try_update(|n| *n += 1);
                        notify_workspace_updated();
                    }
                    Err(message) => {
                        error.try_set(Some(message));
                    }
                }
                busy.try_set(false);
            });
        };
    let resolve_action = move |action_id: String, resolution: ActionResolution| {
        after(Box::pin(async move {
            api::resolve_workspace_action(&action_id, resolution)
                .await
                .map(|_| ())
                .map_err(|e| e.message().to_owned())
        }));
    };
    let set_status = move |status: WorkspaceSubjectStatus| {
        let (w, s) = (workspace_id.get_untracked(), subject_id.get_untracked());
        after(Box::pin(async move {
            api::set_workspace_subject_status(&w, &s, status)
                .await
                .map(|_| ())
                .map_err(|e| e.message().to_owned())
        }));
    };

    let header = move || {
        let (workspace, subject) =
            detail.with(|d| d.as_ref().map(|d| (d.workspace.clone(), d.subject.clone())))?;
        let current = subject.status;
        let options = [
            (WorkspaceSubjectStatus::Active, "Active"),
            (WorkspaceSubjectStatus::Paused, "Paused"),
            (WorkspaceSubjectStatus::Completed, "Completed"),
            (WorkspaceSubjectStatus::Archived, "Archived"),
        ];
        let updated = subject.updated_at as f64;
        let researched = subject.last_researched_at.map(|t| t as f64);
        let (kind, word) = subject_status(current);
        let status_select = ViewFn::from(move || {
            view! {
                <label class="ws-status">
                    <span class="sr-only">"Subject status"</span>
                    <select
                        class="select"
                        aria-label="Subject status"
                        disabled=move || busy.get()
                        on:change=move |event| {
                            let status = match event_target_value(&event).as_str() {
                                "active" => WorkspaceSubjectStatus::Active,
                                "paused" => WorkspaceSubjectStatus::Paused,
                                "completed" => WorkspaceSubjectStatus::Completed,
                                _ => WorkspaceSubjectStatus::Archived,
                            };
                            set_status(status);
                        }
                    >
                        {options
                            .into_iter()
                            .map(|(status, label)| {
                                view! {
                                    <option value=status.as_str() prop:selected=status == current>
                                        {label}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
            }
        });
        Some(view! {
            <div id="workspace-summary">
                <PageHead
                    title=subject.title.clone()
                    lede=subject.summary.clone()
                    actions=status_select
                >
                    <div class="cluster small muted ws-subject-meta">
                        <Link to=format!("/workspaces/{}", encode_uri_component(&workspace.id)) class="textlink">
                            {workspace.title.clone()}
                        </Link>
                        <Status kind=kind label=word />
                        <span title=format_absolute(updated)>
                            {move || format!("Updated {}", format_relative_at(updated, now.get()))}
                        </span>
                        {researched
                            .map(|at| {
                                view! {
                                    <span title=format_absolute(at)>
                                        {move || format!("Researched {}", format_relative_at(at, now.get()))}
                                    </span>
                                }
                            })}
                        {move || {
                            (sending.get() || busy.get())
                                .then(|| view! { <Status kind=StatusKind::Running label="Working" /> })
                        }}
                    </div>
                </PageHead>
            </div>
        })
    };

    let outline = move || {
        let current = section.get();
        let artifacts = artifact_keys.get();
        let current_key = current_artifact.get();
        SubjectSection::ALL
            .into_iter()
            .filter(|s| *s != SubjectSection::Papercuts || count_of(*s) > 0)
            .map(|s| {
                let selected = s == current;
                let count = count_of(s);
                let pending = pending_count();
                let badge = if s == SubjectSection::Actions && pending > 0 {
                    view! { <span class="count warn">{pending}</span> }.into_any()
                } else {
                    view! { <span class="seg-n">{count}</span> }.into_any()
                };
                let subs = (s == SubjectSection::Artifacts && artifacts.len() > 1).then(|| {
                    artifacts
                        .iter()
                        .map(|(key, title)| {
                            let on = selected && current_key.as_deref() == Some(key.as_str());
                            let key = key.clone();
                            view! {
                                <button
                                    type="button"
                                    class="ws-outline-item sub"
                                    aria-current=on.then_some("true")
                                    on:click=move |_| select_artifact(key.clone())
                                >
                                    <span class="truncate">{title.clone()}</span>
                                </button>
                            }
                        })
                        .collect_view()
                });
                view! {
                    <button
                        type="button"
                        class="ws-outline-item"
                        aria-current=selected.then_some("true")
                        on:click=move |_| select_section(s)
                    >
                        <span>{s.label()}</span>
                        {badge}
                    </button>
                    {subs}
                }
            })
            .collect_view()
    };
    let section_options = Signal::derive(move || {
        SubjectSection::ALL
            .into_iter()
            .filter(|s| *s != SubjectSection::Papercuts || count_of(*s) > 0)
            .map(|s| SegOption::new(s, s.label()).with_count(count_of(s)))
            .collect::<Vec<_>>()
    });

    let actions_section = move || {
        let actions = detail.with(|d| d.as_ref().map(|d| d.actions.clone()).unwrap_or_default());
        if actions.is_empty() {
            return view! { <EmptyState compact=true message="Nothing needs your approval for this subject." /> }
                .into_any();
        }
        let (open, done): (Vec<_>, Vec<_>) = actions.into_iter().partition(|a| {
            matches!(
                a.status,
                WorkspaceActionStatus::Pending | WorkspaceActionStatus::Failed
            )
        });
        let card = move |action: WorkspaceAction| {
            let is_open = matches!(
                action.status,
                WorkspaceActionStatus::Pending | WorkspaceActionStatus::Failed
            );
            let pending = action.status == WorkspaceActionStatus::Pending;
            let failed = action.status == WorkspaceActionStatus::Failed;
            let (kind, word) = action_status(action.status);
            let approve_id = action.action_id.clone();
            let reject_id = action.action_id.clone();
            let created = action.created_at as f64;
            view! {
                <Panel
                    id=format!("action-{}", action.action_id)
                    class=if pending { "ws-action pending" } else { "ws-action" }
                >
                    <div class="ws-action-body">
                        <div class="ws-action-head">
                            <h3>{action.title.clone()}</h3>
                            <Status kind=kind label=word />
                        </div>
                        <p class="dim">{action.description.clone()}</p>
                        <span class="mono small muted" title=format_absolute(created)>
                            {move || format!("Proposed {}", format_relative_at(created, now.get()))}
                        </span>
                        {action
                            .result
                            .clone()
                            .filter(|r| !r.is_empty())
                            .map(|r| view! { <p class="ws-action-result small">{r}</p> })}
                    </div>
                    <Disclosure summary="Action details" open=is_open class="ws-action-payload">
                        <pre class="code-block">{format_action_payload(&action.payload)}</pre>
                    </Disclosure>
                    {is_open
                        .then(|| {
                            view! {
                                <div class="ws-action-buttons">
                                    <Button
                                        variant=ButtonVariant::Primary
                                        icon=Icon::Check
                                        busy
                                        on_click=Callback::new(move |_| resolve_action(approve_id.clone(), ActionResolution::Approve))
                                    >
                                        {if failed { "Retry" } else { "Approve" }}
                                    </Button>
                                    {pending
                                        .then(|| {
                                            view! {
                                                <ConfirmButton
                                                    label="Reject"
                                                    confirm_label="Confirm reject"
                                                    variant=ButtonVariant::Danger
                                                    disabled=busy
                                                    disabled_reason="Another change is in progress"
                                                    on_confirm=Callback::new(move |()| resolve_action(reject_id.clone(), ActionResolution::Reject))
                                                />
                                            }
                                        })}
                                </div>
                            }
                        })}
                </Panel>
            }
        };
        let done_count = done.len();
        view! {
            <div class="stack">
                {open.into_iter().map(card).collect_view()}
                {(done_count > 0)
                    .then(|| {
                        view! {
                            <div class="group-head ws-resolved">"Resolved" <span class="seg-n">{done_count}</span></div>
                            {done.into_iter().map(card).collect_view()}
                        }
                    })}
            </div>
        }
        .into_any()
    };

    let revisions = Memo::new(move |_| {
        let mut counts = HashMap::<String, usize>::new();
        detail.with(|d| {
            for item in d.iter().flat_map(|d| d.artifact_revisions.iter()) {
                *counts.entry(item.artifact_key.clone()).or_default() += 1;
            }
        });
        counts
    });
    let artifacts_section = move || {
        let d = detail.get()?;
        let counts = revisions.get();
        let keys = artifact_keys.get();
        let tabs = (keys.len() > 1).then(|| {
            let options = keys
                .iter()
                .map(|(key, title)| SegOption::new(key.clone(), title.clone()))
                .collect::<Vec<_>>();
            let select_options = keys
                .iter()
                .map(|(key, title)| {
                    let key = key.clone();
                    let selected_key = key.clone();
                    view! {
                        <option value=key prop:selected=move || current_artifact.get().as_deref() == Some(selected_key.as_str())>
                            {title.clone()}
                        </option>
                    }
                })
                .collect_view();
            view! {
                <div class="ws-artifact-tabs hide-phone">
                    <Segmented
                        options=Signal::stored(options)
                        value=Signal::derive(move || current_artifact.get().unwrap_or_default())
                        on_change=Callback::new(move |key: String| select_artifact(key))
                        aria_label="Artifact"
                        small=true
                    />
                </div>
                <select
                    class="select only-phone ws-artifact-select"
                    aria-label="Artifact"
                    on:change=move |event| select_artifact(event_target_value(&event))
                >
                    {select_options}
                </select>
            }
        });
        let articles = d
            .workspace
            .artifacts
            .iter()
            .map(|definition| {
                let artifact = d.artifacts.iter().find(|a| a.artifact_key == definition.key);
                let count = counts.get(&definition.key).copied().unwrap_or(0);
                let key = definition.key.clone();
                let hidden = move || current_artifact.get().as_deref() != Some(key.as_str());
                let body = match artifact {
                    Some(artifact) => {
                        let created = artifact.created_at as f64;
                        view! {
                            <div class="ws-artifact-meta mono small muted">
                                <span title=format_absolute(created)>
                                    {move || format!("Updated {}", format_relative_at(created, now.get()))}
                                </span>
                                <span>{plural(count, "revision", "revisions")}</span>
                            </div>
                            <WorkspaceMarkdown content=artifact.content.clone() />
                        }
                        .into_any()
                    }
                    None => view! { <EmptyState compact=true message="Not created yet. The next research run fills it in." /> }
                        .into_any(),
                };
                view! {
                    <article class="ws-artifact" id=format!("artifact-{}", definition.key) hidden=hidden>
                        <h2 class="ws-artifact-title">{definition.title.clone()}</h2>
                        {body}
                    </article>
                }
            })
            .collect_view();
        Some(view! {
            {tabs}
            {articles}
        })
    };

    let conversation = move || {
        let d = detail.get()?;
        let placeholder = d
            .workspace
            .follow_up_placeholder
            .clone()
            .unwrap_or_else(|| {
                format!(
                    "Add details or ask for the next step on this {}…",
                    d.workspace.subject_label.to_lowercase()
                )
            });
        let thread = if d.messages.is_empty() {
            view! { <EmptyState compact=true message="No messages yet." /> }.into_any()
        } else {
            d.messages
                .iter()
                .map(|item| {
                    let (who, class) = match item.role {
                        WorkspaceMessageRole::User => ("You", "ws-msg user"),
                        WorkspaceMessageRole::Assistant => ("Omni", "ws-msg"),
                        WorkspaceMessageRole::System => ("System", "ws-msg system"),
                    };
                    view! {
                        <article class=class>
                            <div class="ws-msg-head">
                                <strong>{who}</strong>
                                <time class="mono small muted">{format_absolute(item.created_at as f64)}</time>
                            </div>
                            <p class="ws-msg-text">{item.text.clone()}</p>
                        </article>
                    }
                })
                .collect_view()
                .into_any()
        };
        Some(view! {
            <div class="ws-thread">{thread}</div>
            <Compose
                workspace_id=workspace_id
                subject_id=Signal::derive(move || Some(subject_id.get()))
                placeholder=placeholder
                label="Message workspace"
                send_label="Send"
                primary=true
                busy=sending
                error
                on_done=Callback::new(move |()| reload.update(|n| *n += 1))
            />
        })
    };

    let sources = move || {
        let d = detail.get()?;
        let scope = d.email_scope.clone().map(|scope| {
            let line = |label: &'static str, items: Vec<String>| {
                (!items.is_empty()).then(|| {
                    view! {
                        <dt>{label}</dt>
                        <dd class="mono">{items.join(", ")}</dd>
                    }
                })
            };
            let lines = view! {
                {line("Senders", scope.senders)}
                {line("Domains", scope.domains)}
                {line("Subject has", scope.subject_keywords)}
                {line("Body has", scope.body_keywords)}
            };
            view! {
                <Panel title="Email scope" pad=true>
                    <dl class="kv">{lines}</dl>
                </Panel>
            }
        });
        let items = d
            .sources
            .iter()
            .map(|source| {
                let title = match safe_source_href(source.url.as_deref()) {
                    Some(href) => view! {
                        <a class="row-title ws-source-link" href=href target="_blank" rel="noreferrer">
                            {source.title.clone()}
                        </a>
                    }
                    .into_any(),
                    None => view! { <span class="row-title">{source.title.clone()}</span> }.into_any(),
                };
                let kind = source_kind_str(source.kind);
                view! {
                    <div class="row ws-source">
                        <Tag>{kind}</Tag>
                        <div class="row-main">
                            {title}
                            <p class="small muted clamp-2">{source.excerpt.clone()}</p>
                        </div>
                    </div>
                }
            })
            .collect_view();
        let list = if d.sources.is_empty() {
            view! { <EmptyState compact=true message="No sources captured yet." /> }.into_any()
        } else {
            view! { <Panel><div class="rows">{items}</div></Panel> }.into_any()
        };
        Some(view! {
            <div class="stack">
                {scope}
                {list}
            </div>
        })
    };

    let papercuts = move || {
        let d = detail.get()?;
        let rows = d
            .papercuts
            .iter()
            .map(|item| {
                view! {
                    <div class="row">
                        <div class="row-main">
                            <span class="row-title">{item.title.clone()}</span>
                            <p class="small muted">{item.detail.clone()}</p>
                        </div>
                        <span class="row-end mono small">{format!("×{}", item.occurrences)}</span>
                    </div>
                }
            })
            .collect_view();
        Some(if d.papercuts.is_empty() {
            view! { <EmptyState compact=true message="No papercuts reported." /> }.into_any()
        } else {
            view! { <Panel><div class="rows">{rows}</div></Panel> }.into_any()
        })
    };

    let pane = move |s: SubjectSection, content: ViewFn| {
        view! {
            <section
                class="ws-pane"
                id=s.as_str()
                hidden=move || section.get() != s
                aria-label=s.label()
            >
                {move || content.run()}
            </section>
        }
    };

    let loaded = Memo::new(move |_| detail.with(Option::is_some));
    move || {
        if !loaded.get() {
            return match error.get() {
                Some(e) => view! {
                    <ErrorState
                        title="This subject could not load"
                        raw=e
                        retry=Callback::new(move |()| reload.update(|n| *n += 1))
                        link=("Back to the workspace".to_owned(), format!("/workspaces/{}", encode_uri_component(&workspace_id.get_untracked())))
                        page=true
                    />
                }
                .into_any(),
                None => view! { <SkeletonRows count=6 label="Loading subject" /> }.into_any(),
            };
        }
        view! {
            {header}
            {move || error.get().map(|e| view! { <ErrorState title="That change did not go through" raw=e /> })}
            <div class="ws-dossier">
                <nav class="ws-outline" aria-label="Subject sections">{outline}</nav>
                <div class="ws-sections-seg">
                    <Segmented
                        options=section_options
                        value=section
                        on_change=Callback::new(select_section)
                        aria_label="Subject sections"
                        small=true
                    />
                </div>
                <div class="ws-panes">
                    {pane(SubjectSection::Actions, ViewFn::from(actions_section))}
                    {pane(SubjectSection::Artifacts, ViewFn::from(artifacts_section))}
                    {pane(SubjectSection::Conversation, ViewFn::from(conversation))}
                    {pane(SubjectSection::Sources, ViewFn::from(sources))}
                    {pane(SubjectSection::Papercuts, ViewFn::from(papercuts))}
                </div>
            </div>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::{SubjectSection, deep_link_selection, format_action_payload, js_stringify_pretty};

    #[test]
    fn action_payloads_pretty_print_like_json_stringify() {
        assert_eq!(
            format_action_payload(r#"{"b":1.0,"a":[1,{"x":"y\n"}],"e":{},"f":[]}"#),
            "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    {\n      \"x\": \"y\\n\"\n    }\n  ],\n  \"e\": {},\n  \"f\": []\n}"
        );
        assert_eq!(
            format_action_payload("not json"),
            "Stored action details are unavailable."
        );
        assert_eq!(js_stringify_pretty(&serde_json::json!(null)), "null");
    }

    #[test]
    fn deep_links_pick_the_section_and_artifact_tab() {
        assert_eq!(
            deep_link_selection(Some("actions"), Some("action-1")),
            (Some(SubjectSection::Actions), None)
        );
        assert_eq!(
            deep_link_selection(None, Some("action-1")),
            (Some(SubjectSection::Actions), None)
        );
        assert_eq!(
            deep_link_selection(Some("artifacts"), Some("artifact-comparison")),
            (
                Some(SubjectSection::Artifacts),
                Some("comparison".to_owned())
            )
        );
        assert_eq!(
            deep_link_selection(None, Some("artifact-comparison")),
            (
                Some(SubjectSection::Artifacts),
                Some("comparison".to_owned())
            )
        );
        assert_eq!(
            deep_link_selection(Some("sources"), None),
            (Some(SubjectSection::Sources), None)
        );
        assert_eq!(deep_link_selection(Some("bogus"), None), (None, None));
        assert_eq!(deep_link_selection(None, None), (None, None));
    }
}
