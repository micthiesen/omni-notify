//! What the parcel and calendar pipelines did with each email.

use std::collections::HashMap;

use leptos::prelude::*;
use omni_api::email::{
    BuiltinRules, EmailActivity, EmailActivityOutcome, EmailFeedback, EmailPipelineName, EmailRule,
    EmailRuleInput, RuleScope, RuleUpsertStatus, RuleVerdict,
};
use omni_web_kit::api;
use omni_web_kit::components::{
    EmailLogModal, ShowMoreButton, StatusFilterChips, Toast, ToastKind, use_show_more, use_toast,
};
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::email_labels::{PIPELINES, outcome_label, pipeline_label};
use omni_web_kit::utils::format::format_absolute;

const OUTCOME_FILTER_ORDER: [EmailActivityOutcome; 7] = [
    EmailActivityOutcome::Processed,
    EmailActivityOutcome::Partial,
    EmailActivityOutcome::Failed,
    EmailActivityOutcome::NoMatches,
    EmailActivityOutcome::Skipped,
    EmailActivityOutcome::Filtered,
    EmailActivityOutcome::Error,
];

fn scope_label(scope: RuleScope) -> &'static str {
    match scope {
        RuleScope::Parcel => "Parcels",
        RuleScope::Calendar => "Calendar",
        RuleScope::Both => "Both",
    }
}

fn verdict_label(verdict: RuleVerdict) -> &'static str {
    match verdict {
        RuleVerdict::Block => "Block",
        RuleVerdict::Allow => "Allow",
    }
}

#[component]
fn BuiltinList(label: &'static str, verdict: RuleVerdict, patterns: Vec<String>) -> impl IntoView {
    view! {
        <div class="rule-builtin-group">
            <div class="rule-builtin-label">
                <span class=format!("rule-verdict rule-verdict-{}", verdict.as_str())>
                    {verdict_label(verdict)}
                </span>
                <span>{label}</span>
            </div>
            <div class="rule-builtin-patterns">
                {patterns
                    .into_iter()
                    .map(|p| view! { <code class="rule-builtin-pattern">{p}</code> })
                    .collect_view()}
            </div>
        </div>
    }
}

/// Collapsible sender rules with delete and an inline add form.
#[component]
fn SenderRulesSection() -> impl IntoView {
    let rules = RwSignal::new(None::<Vec<EmailRule>>);
    let builtin = RwSignal::new(None::<BuiltinRules>);
    let open = RwSignal::new(false);
    let builtin_open = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let pattern = RwSignal::new(String::new());
    let scope = RwSignal::new(RuleScope::Parcel);
    let verdict = RwSignal::new(RuleVerdict::Block);
    let submitting = RwSignal::new(false);
    let toast = use_toast();

    spawn_scoped(async move {
        match api::fetch_email_rules().await {
            Ok(res) => {
                rules.set(Some(res.rules));
                builtin.set(Some(res.builtin));
            }
            Err(e) => error.set(Some(e.message().to_owned())),
        }
    });

    let add_rule = move || {
        let trimmed = pattern.get_untracked().trim().to_lowercase();
        if trimmed.is_empty() || submitting.get_untracked() {
            return;
        }
        submitting.set(true);
        error.set(None);
        let input = EmailRuleInput {
            pattern: trimmed,
            scope: scope.get_untracked(),
            verdict: verdict.get_untracked(),
        };
        spawn_detached(async move {
            match api::create_email_rule(&input).await {
                Ok(res) => {
                    match res.status {
                        RuleUpsertStatus::Created => toast.show("Rule added", ToastKind::Info),
                        RuleUpsertStatus::Merged => toast.show(
                            res.message
                                .clone()
                                .unwrap_or_else(|| "Merged into a single Both rule".into()),
                            ToastKind::Info,
                        ),
                        RuleUpsertStatus::Exists => toast.show(
                            res.message
                                .clone()
                                .unwrap_or_else(|| "That rule already exists".into()),
                            ToastKind::Error,
                        ),
                        RuleUpsertStatus::Builtin => toast.show(
                            res.message
                                .clone()
                                .unwrap_or_else(|| "Already covered by a built-in list".into()),
                            ToastKind::Error,
                        ),
                    }
                    // builtin/exists may return no new rule; prepend only when one landed.
                    if let Some(rule) = res.rule {
                        rules.update(|list| {
                            let mut others: Vec<EmailRule> = list
                                .take()
                                .unwrap_or_default()
                                .into_iter()
                                .filter(|r| r.rule_id != rule.rule_id)
                                .collect();
                            others.insert(0, rule);
                            *list = Some(others);
                        });
                    }
                    pattern.set(String::new());
                }
                Err(e) => error.set(Some(e.message().to_owned())),
            }
            submitting.set(false);
        });
    };

    let remove_rule = move |rule: EmailRule| {
        let message = format!(
            "Delete the {} rule for \"{}\" ({})?",
            rule.verdict.as_str(),
            rule.pattern,
            scope_label(rule.scope)
        );
        if !window().confirm_with_message(&message).unwrap_or(false) {
            return;
        }
        error.set(None);
        spawn_detached(async move {
            match api::delete_email_rule(&rule.rule_id).await {
                Ok(_) => rules.update(|list| {
                    if let Some(list) = list {
                        list.retain(|r| r.rule_id != rule.rule_id);
                    }
                }),
                Err(e) => error.set(Some(e.message().to_owned())),
            }
        });
    };

    let rule_list = move || {
        let list = rules.get();
        match list {
            None => error
                .with(Option::is_none)
                .then(|| view! { <div class="muted mail-rules-note">"Loading rules…"</div> }.into_any()),
            Some(list) if list.is_empty() => Some(
                view! {
                    <div class="muted mail-rules-note">
                        "No sender rules yet. Rules match a full address (x@y.com) or a domain (y.com)."
                    </div>
                }
                .into_any(),
            ),
            Some(list) => Some(
                view! {
                    <ul class="rule-list">
                        {list
                            .into_iter()
                            .map(|rule| {
                                let target = rule.clone();
                                view! {
                                    <li class="rule-row">
                                        <span class=format!("rule-verdict rule-verdict-{}", rule.verdict.as_str())>
                                            {verdict_label(rule.verdict)}
                                        </span>
                                        <span class="rule-pattern" title=rule.pattern.clone()>
                                            {rule.pattern.clone()}
                                        </span>
                                        <span class="rule-scope">{scope_label(rule.scope)}</span>
                                        <button
                                            type="button"
                                            class="rule-delete"
                                            on:click=move |_| remove_rule(target.clone())
                                        >
                                            "Delete"
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                .into_any(),
            ),
        }
    };

    let builtin_section = move || {
        builtin.get().map(|b| {
            let count = b.parcel.blocked.len()
                + b.parcel.auto_pass.len()
                + b.calendar.blocked.len()
                + b.calendar.auto_pass.len();
            let lists = b.clone();
            view! {
                <div class="rule-builtin">
                    <button
                        type="button"
                        class="mail-rules-toggle"
                        aria-expanded=move || builtin_open.get().to_string()
                        on:click=move |_| builtin_open.update(|v| *v = !*v)
                    >
                        <span class="mail-rules-caret">{move || if builtin_open.get() { "▾" } else { "▸" }}</span>
                        "Built-in Lists"
                        <span class="chip-btn-count">{count}</span>
                    </button>
                    {move || {
                        let lists = lists.clone();
                        builtin_open.get().then(|| view! {
                            <div class="rule-builtin-body">
                                <BuiltinList label="Parcels · Blocked" verdict=RuleVerdict::Block patterns=lists.parcel.blocked/>
                                <BuiltinList label="Parcels · Auto-pass" verdict=RuleVerdict::Allow patterns=lists.parcel.auto_pass/>
                                <BuiltinList label="Calendar · Blocked" verdict=RuleVerdict::Block patterns=lists.calendar.blocked/>
                                <BuiltinList label="Calendar · Auto-pass" verdict=RuleVerdict::Allow patterns=lists.calendar.auto_pass/>
                                <div class="muted mail-rules-note">
                                    "Built-ins ship with the app and live in code. An Allow rule above overrides a built-in block for that sender."
                                </div>
                            </div>
                        })
                    }}
                </div>
            }
        })
    };

    view! {
        <section class="mail-rules">
            <Toast toast=toast.toast/>
            <button
                type="button"
                class="mail-rules-toggle"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|v| *v = !*v)
            >
                <span class="mail-rules-caret">{move || if open.get() { "▾" } else { "▸" }}</span>
                "Sender Rules"
                {move || rules.with(|r| r.as_ref().map(Vec::len)).map(|n| view! { <span class="chip-btn-count">{n}</span> })}
            </button>
            <Show when=move || open.get()>
                <div class="mail-rules-body">
                    {rule_list}
                    <form
                        class="rule-form"
                        on:submit=move |event| {
                            event.prevent_default();
                            add_rule();
                        }
                    >
                        <input
                            type="text"
                            class="rule-form-input"
                            placeholder="x@y.com or y.com"
                            prop:value=move || pattern.get()
                            on:input=move |ev| pattern.set(event_target_value(&ev))
                            aria-label="Sender pattern"
                        />
                        <select
                            class="rule-form-select"
                            prop:value=move || scope.get().as_str()
                            on:change=move |ev| {
                                scope.set(match event_target_value(&ev).as_str() {
                                    "calendar" => RuleScope::Calendar,
                                    "both" => RuleScope::Both,
                                    _ => RuleScope::Parcel,
                                })
                            }
                            aria-label="Rule scope"
                        >
                            <option value="parcel">"Parcels"</option>
                            <option value="calendar">"Calendar"</option>
                            <option value="both">"Both"</option>
                        </select>
                        <select
                            class="rule-form-select"
                            prop:value=move || verdict.get().as_str()
                            on:change=move |ev| {
                                verdict.set(if event_target_value(&ev) == "allow" { RuleVerdict::Allow } else { RuleVerdict::Block })
                            }
                            aria-label="Rule verdict"
                        >
                            <option value="block">"Block"</option>
                            <option value="allow">"Allow"</option>
                        </select>
                        <button
                            type="submit"
                            class="run-btn"
                            disabled=move || pattern.get().trim().is_empty() || submitting.get()
                        >
                            {move || if submitting.get() { "Adding…" } else { "Add" }}
                        </button>
                    </form>
                    {move || error.get().map(|e| view! { <div class="error-inline mail-rules-error">{e}</div> })}
                    {builtin_section}
                </div>
            </Show>
        </section>
    }
}

#[component]
pub fn EmailActivityPage() -> impl IntoView {
    let activities = RwSignal::new(None::<Vec<EmailActivity>>);
    let error = RwSignal::new(None::<String>);
    let pipeline = RwSignal::new(None::<EmailPipelineName>);
    let outcome = RwSignal::new(String::new());
    let feedback = RwSignal::new(HashMap::<String, EmailFeedback>::new());
    let logs_for = RwSignal::new(None::<EmailActivity>);

    Effect::new(move |_| {
        let selected = pipeline.get();
        activities.set(None);
        error.set(None);
        spawn_scoped(async move {
            match api::fetch_email_activity(selected, Some(500)).await {
                Ok(res) => activities.set(Some(res.activities)),
                Err(e) => error.set(Some(e.message().to_owned())),
            }
        });
    });
    // Feedback indicators are decorative; the page works without them.
    spawn_scoped(async move {
        if let Ok(res) = api::fetch_email_feedback().await {
            feedback.set(
                res.feedback
                    .into_iter()
                    .map(|f| (f.activity_id.clone(), f))
                    .collect(),
            );
        }
    });

    let outcome_counts = Memo::new(move |_| {
        let mut counts = HashMap::new();
        activities.with(|list| {
            for activity in list.iter().flatten() {
                *counts
                    .entry(activity.outcome.as_str().to_owned())
                    .or_insert(0usize) += 1;
            }
        });
        counts
    });
    let filtered = Memo::new(move |_| {
        let selected = outcome.get();
        activities.with(|list| {
            list.iter()
                .flatten()
                .filter(|a| selected.is_empty() || a.outcome.as_str() == selected)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let shown = use_show_more(
        Signal::derive(move || filtered.get()),
        30,
        Signal::derive(move || {
            format!(
                "{}:{}",
                pipeline.get().map_or("all", |p| p.as_str()),
                outcome.get()
            )
        }),
    );

    let on_activity_change = Callback::new(move |updated: EmailActivity| {
        activities.update(|list| {
            if let Some(list) = list {
                for a in list.iter_mut() {
                    if a.activity_id == updated.activity_id {
                        *a = updated.clone();
                    }
                }
            }
        });
    });
    let on_feedback_change =
        Callback::new(move |(id, updated): (String, Option<EmailFeedback>)| {
            feedback.update(|map| match updated {
                Some(f) => {
                    map.insert(id, f);
                }
                None => {
                    map.remove(&id);
                }
            });
        });

    let pipeline_chips = PIPELINES
        .into_iter()
        .map(|p| {
            view! {
                <button
                    type="button"
                    class=move || format!("chip-btn {}", if pipeline.get() == Some(p) { "active" } else { "" })
                    aria-pressed=move || (pipeline.get() == Some(p)).to_string()
                    on:click=move |_| pipeline.set(if pipeline.get_untracked() == Some(p) { None } else { Some(p) })
                >
                    {pipeline_label(p)}
                </button>
            }
        })
        .collect_view();

    let rows = move || {
        shown
            .visible
            .get()
            .into_iter()
            .map(|activity| {
                let clicked = activity.clone();
                let id = activity.activity_id.clone();
                let subject = if activity.subject.is_empty() { "(no subject)".to_owned() } else { activity.subject.clone() };
                view! {
                    <li class="mail-row">
                        <button
                            type="button"
                            class="mail-row-btn"
                            title="View processing logs"
                            on:click=move |_| logs_for.set(Some(clicked.clone()))
                        >
                            <div class="mail-row-top">
                                <span class="mail-subject" title=activity.subject.clone()>{subject}</span>
                                <span class=format!("mail-outcome mail-outcome-{}", activity.outcome.as_str())>
                                    {outcome_label(activity.outcome)}
                                </span>
                            </div>
                            <div class="mail-row-meta">
                                <span class="briefing-badge">{pipeline_label(activity.pipeline)}</span>
                                <span class="mail-from" title=activity.from.clone()>{activity.from.clone()}</span>
                                {move || feedback.with(|f| f.contains_key(&id)).then(|| view! { <span class="mail-feedback-tag">"Feedback"</span> })}
                                <span class="mail-time">{format_absolute(activity.processed_at as f64)}</span>
                            </div>
                            {activity.detail.clone().filter(|d| !d.is_empty()).map(|d| view! { <div class="mail-detail">{d}</div> })}
                            {(!activity.items.is_empty()).then(|| view! {
                                <ul class="mail-items">
                                    {activity.items.iter().map(|item| view! { <li>{item.clone()}</li> }).collect_view()}
                                </ul>
                            })}
                        </button>
                    </li>
                }
            })
            .collect_view()
    };

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"Email Activity"</h1>
                <p class="page-subtitle">"What the parcel and calendar pipelines did with each email."</p>
            </div>
        </div>

        <div class="rec-filters">
            <button
                type="button"
                class=move || format!("chip-btn {}", if pipeline.get().is_none() { "active" } else { "" })
                aria-pressed=move || pipeline.get().is_none().to_string()
                on:click=move |_| pipeline.set(None)
            >
                "All"
            </button>
            {pipeline_chips}
        </div>

        {move || {
            activities.with(|a| a.as_ref().is_some_and(|a| !a.is_empty())).then(|| {
                view! {
                    <StatusFilterChips
                        order=OUTCOME_FILTER_ORDER
                            .iter()
                            .map(|o| (o.as_str().to_owned(), outcome_label(*o).to_owned()))
                            .collect()
                        counts=outcome_counts
                        total=Signal::derive(move || activities.with(|a| a.as_ref().map_or(0, Vec::len)))
                        active=outcome
                        on_change=Callback::new(move |value: String| outcome.set(value))
                    />
                }
            })
        }}

        {move || {
            let state = activities.with(|a| a.as_ref().map(Vec::is_empty));
            match (state, error.get()) {
                (None, None) => Some(view! { <div class="loading">"Loading…"</div> }.into_any()),
                (None, Some(err)) => Some(view! {
                    <div class="error">
                        <div>"Failed to load email activity"</div>
                        <div class="error-detail">{err}</div>
                    </div>
                }.into_any()),
                (Some(true), _) => Some(view! {
                    <div class="muted">
                        "No email activity recorded yet. Activity appears here as new emails are processed."
                    </div>
                }.into_any()),
                (Some(false), _) => filtered
                    .with(Vec::is_empty)
                    .then(|| view! { <div class="muted">"No emails match the current filters."</div> }.into_any()),
            }
        }}

        {move || filtered.with(|f| !f.is_empty()).then(|| view! { <ul class="mail-list">{rows}</ul> })}
        {move || {
            shown.has_more.get().then(|| view! {
                <ShowMoreButton remaining=shown.remaining on_click=Callback::new(move |()| shown.show_more())/>
            })
        }}
        <SenderRulesSection/>
        {move || {
            logs_for.get().map(|activity| {
                let current_feedback = feedback.with_untracked(|f| f.get(&activity.activity_id).cloned());
                match current_feedback {
                    Some(feedback) => view! {
                        <EmailLogModal
                            activity
                            feedback
                            on_activity_change
                            on_feedback_change
                            on_close=Callback::new(move |()| logs_for.set(None))
                        />
                    }
                    .into_any(),
                    None => view! {
                        <EmailLogModal
                            activity
                            on_activity_change
                            on_feedback_change
                            on_close=Callback::new(move |()| logs_for.set(None))
                        />
                    }
                    .into_any(),
                }
            })
        }}
    }
}
