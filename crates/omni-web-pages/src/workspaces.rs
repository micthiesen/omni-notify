//! Workspaces: the overview grid, one workspace, and one subject's dossier
//! (`pages/WorkspacesPage.tsx`).

use std::collections::HashMap;
use std::time::Duration;

use futures::future::{Either, select};
use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::runs::RunStatus;
use omni_api::workspaces::{
    WorkspaceActionStatus, WorkspaceMessageRole, WorkspaceOverview, WorkspaceSourceKind,
    WorkspaceSubjectResponse, WorkspaceSubjectStatus,
};
use omni_web_kit::api::{self, ActionResolution, ApiClientError};
use omni_web_kit::markdown::WorkspaceMarkdown;
use omni_web_kit::router::Link;
use omni_web_kit::task::{sleep, spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute, format_relative};
use omni_web_kit::utils::js::number_string;
use serde_json::Value;

const RUN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

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

/// `formatActionPayload`: the stored JSON, pretty-printed.
pub fn format_action_payload(payload: &str) -> String {
    match serde_json::from_str::<Value>(payload) {
        Ok(value) => js_stringify_pretty(&value),
        Err(_) => "Stored action details are unavailable.".to_owned(),
    }
}

/// `safeSourceHref`: only http(s) URLs, normalized by the URL parser.
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
    let subject_mode =
        Memo::new(move |_| workspace_id.with(Option::is_some) && subject_id.with(Option::is_some));
    move || {
        if subject_mode.get() {
            let workspace = Signal::derive(move || workspace_id.get().unwrap_or_default());
            let subject = Signal::derive(move || subject_id.get().unwrap_or_default());
            view! { <SubjectPage workspace_id=workspace subject_id=subject /> }.into_any()
        } else {
            view! { <WorkspaceList workspace_id=workspace_id /> }.into_any()
        }
    }
}

async fn load_list(workspace_id: Option<String>) -> Result<Vec<WorkspaceOverview>, ApiClientError> {
    match workspace_id {
        Some(id) => {
            let response = api::fetch_workspace(&id).await?;
            let active = response
                .subjects
                .iter()
                .filter(|s| s.status == WorkspaceSubjectStatus::Active)
                .count();
            let pending = response
                .actions
                .iter()
                .filter(|a| a.status == WorkspaceActionStatus::Pending)
                .count();
            Ok(vec![WorkspaceOverview {
                definition: response.workspace,
                subjects: response.subjects,
                active_subject_count: active as u64,
                pending_action_count: pending as u64,
                open_papercut_count: response.papercuts.len() as u64,
            }])
        }
        None => Ok(api::fetch_workspaces().await?.workspaces),
    }
}

#[component]
fn WorkspaceList(#[prop(into)] workspace_id: Signal<Option<String>>) -> impl IntoView {
    let workspaces = RwSignal::new(None::<Vec<WorkspaceOverview>>);
    let messages = RwSignal::new(HashMap::<String, String>::new());
    let busy_workspace_id = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        let id = workspace_id.get();
        spawn_scoped(async move {
            match load_list(id).await {
                Ok(list) => workspaces.set(Some(list)),
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    });

    let send = move |id: String| {
        let message = messages.with_untracked(|m| m.get(&id).map(|m| m.trim().to_owned()));
        let Some(message) = message.filter(|m| !m.is_empty()) else {
            return;
        };
        if busy_workspace_id.with_untracked(Option::is_some) {
            return;
        }
        busy_workspace_id.set(Some(id.clone()));
        error.set(None);
        let scope = workspace_id.get_untracked();
        spawn_detached(async move {
            let result: Result<(), String> = async {
                let accepted = api::send_workspace_message(&id, &message, None)
                    .await
                    .map_err(|e| e.message().to_owned())?;
                wait_for_run(&accepted.run_id).await?;
                messages.update(|m| {
                    m.insert(id.clone(), String::new());
                });
                let list = load_list(scope).await.map_err(|e| e.message().to_owned())?;
                workspaces.set(Some(list));
                notify_workspace_updated();
                Ok(())
            }
            .await;
            if let Err(message) = result {
                error.set(Some(message));
            }
            busy_workspace_id.set(None);
        });
    };

    let card = move |workspace: WorkspaceOverview| {
        let def = workspace.definition;
        let id = def.id.clone();
        let label = def.subject_label.to_lowercase();
        let subjects = if workspace.subjects.is_empty() {
            view! {
                <div class="muted">{format!("Message the workspace to start the first {label}.")}</div>
            }
            .into_any()
        } else {
            workspace
                .subjects
                .iter()
                .map(|subject| {
                    let to = format!(
                        "/workspaces/{}/{}",
                        encode_uri_component(&def.id),
                        encode_uri_component(&subject.subject_id)
                    );
                    let summary = if subject.summary.is_empty() {
                        "Work just started.".to_owned()
                    } else {
                        subject.summary.clone()
                    };
                    let status = subject.status.as_str();
                    let title = subject.title.clone();
                    view! {
                        <Link to=to class="workspace-subject-row">
                            <span>
                                <strong>{title}</strong>
                                <small>{summary}</small>
                            </span>
                            <span class=format!("workspace-status status-{status}")>{status}</span>
                        </Link>
                    }
                })
                .collect_view()
                .into_any()
        };
        let placeholder = def
            .input_placeholder
            .clone()
            .unwrap_or_else(|| format!("What would you like help with for this {label}?"));
        let value_id = id.clone();
        let input_id = id.clone();
        let busy_id = id.clone();
        let disabled_id = id.clone();
        let submit_id = id.clone();
        view! {
            <section class="workspace-card">
                <div class="workspace-card-header">
                    <div>
                        <h2>{def.title.clone()}</h2>
                        <p>{def.description.clone()}</p>
                    </div>
                    <div class="workspace-counts meta-row">
                        <span>{format!("{} Active", workspace.active_subject_count)}</span>
                        <span>{format!("{} Pending", workspace.pending_action_count)}</span>
                        <span>{format!("{} Papercuts", workspace.open_papercut_count)}</span>
                        {(def.scheduled_runs == Some(false)).then(|| view! { <span>"On Demand"</span> })}
                    </div>
                </div>
                <div class="workspace-subject-list">{subjects}</div>
                <form
                    class="workspace-compose"
                    on:submit=move |event: leptos::ev::SubmitEvent| {
                        event.prevent_default();
                        send(submit_id.clone());
                    }
                >
                    <textarea
                        prop:value=move || messages.with(|m| m.get(&value_id).cloned().unwrap_or_default())
                        on:input=move |event| {
                            let text = event_target_value(&event);
                            messages.update(|m| {
                                m.insert(input_id.clone(), text);
                            });
                        }
                        placeholder=placeholder
                        aria-label=format!("Message {}", def.title)
                    ></textarea>
                    <button
                        type="submit"
                        disabled=move || {
                            busy_workspace_id.with(Option::is_some)
                                || messages.with(|m| m.get(&disabled_id).is_none_or(|t| t.trim().is_empty()))
                        }
                    >
                        {move || {
                            if busy_workspace_id.with(|b| b.as_deref() == Some(busy_id.as_str())) {
                                "Starting…"
                            } else {
                                "Send"
                            }
                        }}
                    </button>
                </form>
                {move || {
                    workspace_id
                        .with(Option::is_none)
                        .then(|| {
                            view! {
                                <Link to=format!("/workspaces/{id}") class="workspace-open-link">
                                    "Open Workspace ›"
                                </Link>
                            }
                        })
                }}
            </section>
        }
    };

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>
                    {move || {
                        if workspace_id.with(Option::is_some) {
                            workspaces
                                .with(|w| w.as_ref().and_then(|w| w.first()).map(|w| w.definition.title.clone()))
                                .unwrap_or_else(|| "Workspace".to_owned())
                        } else {
                            "Workspaces".to_owned()
                        }
                    }}
                </h1>
                <p class="page-subtitle">"Research, decisions, and next steps for your ongoing projects."</p>
            </div>
        </div>
        {move || error.get().map(|e| view! { <div class="error">{e}</div> })}
        {move || {
            (workspaces.with(Option::is_none) && error.with(Option::is_none))
                .then(|| view! { <div class="loading">"Loading…"</div> })
        }}
        <div class="workspace-grid">
            <For
                each=move || workspaces.get().unwrap_or_default()
                key=|w| format!("{}|{w:?}", w.definition.id)
                children=card
            />
        </div>
    }
}

#[component]
fn SubjectPage(
    #[prop(into)] workspace_id: Signal<String>,
    #[prop(into)] subject_id: Signal<String>,
) -> impl IntoView {
    let detail = RwSignal::new(None::<WorkspaceSubjectResponse>);
    let message = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    let reload = move || {
        let (w, s) = (workspace_id.get_untracked(), subject_id.get_untracked());
        async move { api::fetch_workspace_subject(&w, &s).await }
    };

    Effect::new(move |_| {
        let (w, s) = (workspace_id.get(), subject_id.get());
        spawn_scoped(async move {
            match api::fetch_workspace_subject(&w, &s).await {
                Ok(response) => detail.set(Some(response)),
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    });

    // `?target=` deep links (Pushover) scroll to and highlight an element.
    Effect::new(move |_| {
        if detail.with(Option::is_none) {
            return;
        }
        request_animation_frame(move || {
            let Some(target) = omni_web_kit::hooks::query_param("target").filter(|t| !t.is_empty())
            else {
                return;
            };
            let Some(element) = document().get_element_by_id(&target) else {
                return;
            };
            let _ = element.class_list().add_1("deep-link-target");
            omni_web_kit::hooks::scroll_into_view_center(&target);
        });
    });

    let revisions = Memo::new(move |_| {
        let mut counts = HashMap::<String, usize>::new();
        detail.with(|d| {
            for item in d.iter().flat_map(|d| d.artifact_revisions.iter()) {
                *counts.entry(item.artifact_key.clone()).or_default() += 1;
            }
        });
        counts
    });

    // Runs `action`, then reloads and notifies; errors land in `error`.
    let after = move |clear_error: bool,
                      action: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), String>>>,
    >| {
        busy.set(true);
        if clear_error {
            error.set(None);
        }
        let next = reload();
        spawn_detached(async move {
            let result: Result<(), String> = async {
                action.await?;
                let response = next.await.map_err(|e| e.message().to_owned())?;
                detail.set(Some(response));
                notify_workspace_updated();
                Ok(())
            }
            .await;
            if let Err(message) = result {
                error.set(Some(message));
            }
            busy.set(false);
        });
    };

    let send = move || {
        let text = message.get_untracked().trim().to_owned();
        if text.is_empty() || busy.get_untracked() {
            return;
        }
        let (w, s) = (workspace_id.get_untracked(), subject_id.get_untracked());
        after(
            true,
            Box::pin(async move {
                let accepted = api::send_workspace_message(&w, &text, Some(&s))
                    .await
                    .map_err(|e| e.message().to_owned())?;
                wait_for_run(&accepted.run_id).await?;
                message.set(String::new());
                Ok(())
            }),
        );
    };

    let resolve_action = move |action_id: String, resolution: ActionResolution| {
        after(
            false,
            Box::pin(async move {
                api::resolve_workspace_action(&action_id, resolution)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.message().to_owned())
            }),
        );
    };

    let set_status = move |status: WorkspaceSubjectStatus| {
        let (w, s) = (workspace_id.get_untracked(), subject_id.get_untracked());
        after(
            true,
            Box::pin(async move {
                api::set_workspace_subject_status(&w, &s, status)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.message().to_owned())
            }),
        );
    };

    let header = move || {
        detail.with(|d| d.as_ref().map(|d| (d.workspace.title.clone(), d.subject.clone())))
            .map(|(workspace_title, subject)| {
                let current = subject.status;
                let options = [
                    (WorkspaceSubjectStatus::Active, "Active"),
                    (WorkspaceSubjectStatus::Paused, "Paused"),
                    (WorkspaceSubjectStatus::Completed, "Completed"),
                    (WorkspaceSubjectStatus::Archived, "Archived"),
                ];
                view! {
                    <div class="page-header" id="workspace-summary">
                        <div class="page-header-stack">
                            <Link to=format!("/workspaces/{}", workspace_id.get_untracked()) class="workspace-back">
                                {format!("← {workspace_title}")}
                            </Link>
                            <h1>{subject.title.clone()}</h1>
                            <p class="page-subtitle">{subject.summary.clone()}</p>
                        </div>
                        <select
                            class="workspace-status-select"
                            aria-label="Subject status"
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
                    </div>
                }
            })
    };

    let actions_section = move || {
        let actions = detail.with(|d| d.as_ref().map(|d| d.actions.clone()).unwrap_or_default());
        (!actions.is_empty()).then(|| {
            view! {
                <section class="workspace-section" id="actions">
                    <h2>"Actions"</h2>
                    <div class="workspace-action-list">
                        {actions
                            .into_iter()
                            .map(|action| {
                                let open = matches!(
                                    action.status,
                                    WorkspaceActionStatus::Pending | WorkspaceActionStatus::Failed
                                );
                                let pending = action.status == WorkspaceActionStatus::Pending;
                                let failed = action.status == WorkspaceActionStatus::Failed;
                                let approve_id = action.action_id.clone();
                                let reject_id = action.action_id.clone();
                                view! {
                                    <article class="workspace-action-card" id=format!("action-{}", action.action_id)>
                                        <div class="workspace-action-heading">
                                            <div>
                                                <strong>{action.title.clone()}</strong>
                                                <p>{action.description.clone()}</p>
                                            </div>
                                            <span class=format!("workspace-status status-{}", action.status.as_str())>
                                                {action.status.as_str()}
                                            </span>
                                        </div>
                                        <details class="content-disclosure" open=open>
                                            <summary>"Action Details"</summary>
                                            <pre>{format_action_payload(&action.payload)}</pre>
                                        </details>
                                        {action
                                            .result
                                            .clone()
                                            .filter(|r| !r.is_empty())
                                            .map(|r| view! { <p class="workspace-action-result">{r}</p> })}
                                        {open
                                            .then(|| {
                                                view! {
                                                    <div class="workspace-action-buttons">
                                                        <button
                                                            type="button"
                                                            on:click=move |_| resolve_action(approve_id.clone(), ActionResolution::Approve)
                                                            disabled=move || busy.get()
                                                        >
                                                            {if failed { "Retry" } else { "Approve" }}
                                                        </button>
                                                        {pending
                                                            .then(|| {
                                                                view! {
                                                                    <button
                                                                        type="button"
                                                                        class="danger-button"
                                                                        on:click=move |_| resolve_action(reject_id.clone(), ActionResolution::Reject)
                                                                        disabled=move || busy.get()
                                                                    >
                                                                        "Reject"
                                                                    </button>
                                                                }
                                                            })}
                                                    </div>
                                                }
                                            })}
                                    </article>
                                }
                            })
                            .collect_view()}
                    </div>
                </section>
            }
        })
    };

    let compose_placeholder = move || {
        detail.with(|d| {
            d.as_ref().map(|d| {
                d.workspace
                    .follow_up_placeholder
                    .clone()
                    .unwrap_or_else(|| {
                        format!(
                            "Add details or ask for the next step on this {}…",
                            d.workspace.subject_label.to_lowercase()
                        )
                    })
            })
        })
    };

    let artifacts = move || {
        detail.with(|d| {
            let d = d.as_ref()?;
            let counts = revisions.get();
            Some(
                d.workspace
                    .artifacts
                    .iter()
                    .map(|definition| {
                        let artifact = d.artifacts.iter().find(|a| a.artifact_key == definition.key);
                        let count = counts.get(&definition.key).copied().unwrap_or(0);
                        let body = match artifact {
                            Some(artifact) => view! {
                                <WorkspaceMarkdown content=artifact.content.clone() />
                                <small>{format!("Updated {}", format_relative(artifact.created_at as f64))}</small>
                            }
                            .into_any(),
                            None => view! { <p class="muted">"Not created yet."</p> }.into_any(),
                        };
                        view! {
                            <article class="workspace-artifact-card" id=format!("artifact-{}", definition.key)>
                                <div class="workspace-artifact-heading">
                                    <h3>{definition.title.clone()}</h3>
                                    {artifact
                                        .is_some()
                                        .then(|| {
                                            view! {
                                                <span>
                                                    {format!("{count} revision{}", if count == 1 { "" } else { "s" })}
                                                </span>
                                            }
                                        })}
                                </div>
                                {body}
                            </article>
                        }
                    })
                    .collect_view(),
            )
        })
    };

    let conversation = move || {
        detail.with(|d| {
            d.as_ref().map(|d| {
                d.messages
                    .iter()
                    .map(|item| {
                        let role = item.role.as_str();
                        let who = if item.role == WorkspaceMessageRole::User {
                            "You"
                        } else {
                            "Omni"
                        };
                        view! {
                            <article class=format!("workspace-message workspace-message-{role}")>
                                <strong>{who}</strong>
                                <p>{item.text.clone()}</p>
                                <small>{format_absolute(item.created_at as f64)}</small>
                            </article>
                        }
                    })
                    .collect_view()
            })
        })
    };

    let sources = move || {
        detail.with(|d| {
            let d = d.as_ref()?;
            let scope = d.email_scope.as_ref().map(|scope| {
                view! {
                    <div class="workspace-scope">
                        <strong>"Email Scope"</strong>
                        <code>{serde_json::to_string(scope).unwrap_or_default()}</code>
                    </div>
                }
            });
            let items = d
                .sources
                .iter()
                .map(|source| {
                    let link = match safe_source_href(source.url.as_deref()) {
                        Some(href) => view! {
                            <a href=href target="_blank" rel="noreferrer">{source.title.clone()}</a>
                        }
                        .into_any(),
                        None => view! { <strong>{source.title.clone()}</strong> }.into_any(),
                    };
                    view! {
                        <article>
                            <span class="workspace-source-kind">{source_kind_str(source.kind)}</span>
                            {link}
                            <p>{source.excerpt.clone()}</p>
                        </article>
                    }
                })
                .collect_view();
            let empty = d
                .sources
                .is_empty()
                .then(|| view! { <p class="muted">"No sources captured yet."</p> });
            Some(view! {
                {scope}
                <div class="workspace-source-list">{items} {empty}</div>
            })
        })
    };

    let papercuts = move || {
        detail.with(|d| {
            let d = d.as_ref()?;
            (!d.papercuts.is_empty()).then(|| {
                view! {
                    <section class="workspace-section workspace-papercuts">
                        <h2>"Papercuts"</h2>
                        {d
                            .papercuts
                            .iter()
                            .map(|item| {
                                view! {
                                    <article>
                                        <strong>{item.title.clone()}</strong>
                                        <p>{item.detail.clone()}</p>
                                        <small>
                                            {format!(
                                                "{} occurrence{}",
                                                item.occurrences,
                                                if item.occurrences == 1 { "" } else { "s" },
                                            )}
                                        </small>
                                    </article>
                                }
                            })
                            .collect_view()}
                    </section>
                }
            })
        })
    };

    let loaded = Memo::new(move |_| detail.with(Option::is_some));
    move || {
        if !loaded.get() {
            return match error.get() {
                Some(e) => view! { <div class="error">{e}</div> }.into_any(),
                None => view! { <div class="loading">"Loading…"</div> }.into_any(),
            };
        }
        view! {
            {header}
            {move || error.get().map(|e| view! { <div class="error">{e}</div> })}
            {actions_section}
            <form
                class="workspace-compose workspace-compose-detail"
                on:submit=move |event: leptos::ev::SubmitEvent| {
                    event.prevent_default();
                    send();
                }
            >
                <textarea
                    prop:value=move || message.get()
                    on:input=move |event| message.set(event_target_value(&event))
                    placeholder=compose_placeholder
                    aria-label="Message workspace"
                ></textarea>
                <button type="submit" disabled=move || busy.get() || message.with(|m| m.trim().is_empty())>
                    {move || if busy.get() { "Working…" } else { "Send" }}
                </button>
            </form>

            <nav class="workspace-section-nav" aria-label="Project sections">
                <a href="#artifacts">"Research"</a>
                <a href="#conversation">"Conversation"</a>
                <a href="#sources">"Sources"</a>
            </nav>
            <section class="workspace-section" id="artifacts">
                <h2>"Artifacts"</h2>
                <div class="workspace-artifact-grid">{artifacts}</div>
            </section>

            <div class="workspace-two-column">
                <section class="workspace-section" id="conversation">
                    <h2>"Conversation"</h2>
                    <div class="workspace-message-list">{conversation}</div>
                </section>
                <section class="workspace-section" id="sources">
                    <h2>"Sources"</h2>
                    {sources}
                </section>
            </div>
            {papercuts}
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::{format_action_payload, js_stringify_pretty};

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
}
