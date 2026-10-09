//! Email processing logs plus actions: reprocess, feedback, block sender and
//! per-item parcel "forget".

use std::collections::HashSet;

use leptos::prelude::*;
use omni_api::email::{
    AdmitTier, BuiltinRules, EmailActivity, EmailFeedback, EmailFeedbackVerdict, EmailPipelineName,
    EmailRule, EmailRuleInput, RuleScope, RuleUpsertStatus, RuleVerdict,
};
use omni_api::runs::RunLogLine;

use super::log_viewer::LogLines;
use super::toast::{Toast, ToastKind, use_toast};
use crate::api;
use crate::hooks::use_modal;
use crate::task::{spawn_detached, spawn_scoped};
use crate::utils::email_labels::{outcome_label, pipeline_label};
use crate::utils::format::{format_absolute, format_cents};

fn admit_tier_label(tier: AdmitTier) -> &'static str {
    match tier {
        AdmitTier::Rule => "Rule",
        AdmitTier::Builtin => "Built-in",
        AdmitTier::Triage => "Triage",
        AdmitTier::KeywordFallback => "Keyword Fallback",
        AdmitTier::CarrierName => "Carrier Name",
    }
}

fn feedback_label(pipeline: EmailPipelineName, verdict: EmailFeedbackVerdict) -> &'static str {
    match (pipeline, verdict) {
        (EmailPipelineName::ParcelTracker, EmailFeedbackVerdict::NotRelevant) => "Not a Parcel",
        (EmailPipelineName::ParcelTracker, EmailFeedbackVerdict::Missed) => "Missed Parcel",
        (EmailPipelineName::CalendarEvents, EmailFeedbackVerdict::NotRelevant) => "Not an Event",
        (EmailPipelineName::CalendarEvents, EmailFeedbackVerdict::Missed) => "Missed Event",
    }
}

fn block_scope(pipeline: EmailPipelineName) -> RuleScope {
    match pipeline {
        EmailPipelineName::ParcelTracker => RuleScope::Parcel,
        EmailPipelineName::CalendarEvents => RuleScope::Calendar,
    }
}

/// Item lines look like "<token> (...)" or "<token>: ..."; the leading token
/// is the tracking number (`/^(\S+?)(?::|\s+\()/`).
pub fn extract_tracking_number(item: &str) -> Option<String> {
    let chars: Vec<char> = item.chars().collect();
    for end in 1..=chars.len() {
        if chars[end - 1].is_whitespace() {
            return None;
        }
        let rest = &chars[end..];
        if rest.first() == Some(&':') {
            return Some(chars[..end].iter().collect());
        }
        let spaces = rest.iter().take_while(|c| c.is_whitespace()).count();
        if spaces > 0 && rest.get(spaces) == Some(&'(') {
            return Some(chars[..end].iter().collect());
        }
    }
    None
}

/// The lowercased address of a bare or display-name `From`.
pub fn extract_sender_email(from: &str) -> String {
    let inner = from
        .find('<')
        .and_then(|start| {
            let rest = &from[start + 1..];
            let end = rest.find('>')?;
            (end > 0).then(|| &rest[..end])
        })
        .unwrap_or(from);
    inner.trim().to_lowercase()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coverage {
    Rule,
    Builtin,
}

fn confirm(message: &str) -> bool {
    window().confirm_with_message(message).unwrap_or(false)
}

#[component]
pub fn EmailLogModal(
    activity: EmailActivity,
    #[prop(optional)] feedback: Option<EmailFeedback>,
    #[prop(optional)] on_activity_change: Option<Callback<EmailActivity>>,
    #[prop(optional)] on_feedback_change: Option<Callback<(String, Option<EmailFeedback>)>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let current = RwSignal::new(activity);
    let lines = RwSignal::new(None::<Vec<RunLogLine>>);
    let dropped = RwSignal::new(0u64);
    let error = RwSignal::new(None::<String>);
    let logs_version = RwSignal::new(0u64);
    let reprocessing = RwSignal::new(false);
    let fb = RwSignal::new(feedback);
    let fb_busy = RwSignal::new(false);
    let blocking = RwSignal::new(false);
    let block_scope_value = RwSignal::new(RuleScope::Both);
    let block_exact = RwSignal::new(false);
    let forgetting = RwSignal::new(None::<String>);
    let forgotten = RwSignal::new(HashSet::<String>::new());
    let action_error = RwSignal::new(None::<String>);
    let toast = use_toast();
    let rules = RwSignal::new(None::<Vec<EmailRule>>);
    let builtin = RwSignal::new(None::<BuiltinRules>);
    let coverage_override = RwSignal::new(None::<Coverage>);

    let activity_id = Memo::new(move |_| current.with(|a| a.activity_id.clone()));
    Effect::new(move |_| {
        let id = activity_id.get();
        logs_version.track();
        lines.set(None);
        error.set(None);
        spawn_scoped(async move {
            match api::fetch_email_activity_logs(&id).await {
                Ok(data) => {
                    lines.set(Some(data.lines));
                    dropped.set(data.dropped);
                }
                Err(e) => error.set(Some(e.message().to_owned())),
            }
        });
    });

    let modal_ref = use_modal(move || on_close.run(()));

    // Sender-rule coverage is decorative: the block button works without it.
    spawn_scoped(async move {
        if let Ok(res) = api::fetch_email_rules().await {
            rules.set(Some(res.rules));
            builtin.set(Some(res.builtin));
        }
    });

    let pipeline = move || current.with(|a| a.pipeline);
    let sender_email = Memo::new(move |_| current.with(|a| extract_sender_email(&a.from)));
    let sender_domain =
        Memo::new(move |_| sender_email.with(|e| e.split('@').nth(1).unwrap_or("").to_owned()));
    let sender_at_domain = Memo::new(move |_| {
        sender_domain.with(|d| {
            if d.is_empty() {
                String::new()
            } else {
                format!("@{d}")
            }
        })
    });

    let coverage = Memo::new(move |_| {
        if let Some(over) = coverage_override.get() {
            return Some(over);
        }
        let email = sender_email.get();
        let domain = sender_domain.get();
        let at_domain = sender_at_domain.get();
        // Rules may be stored as the bare domain, "@domain" or the full address.
        let matches = |pattern: &str| pattern == email || pattern == domain || pattern == at_domain;
        let scope = block_scope(pipeline());
        let by_rule = rules.with(|rules| {
            rules.as_ref().is_some_and(|rules| {
                rules.iter().any(|r| {
                    r.verdict == RuleVerdict::Block
                        && (r.scope == RuleScope::Both || r.scope == scope)
                        && matches(&r.pattern)
                })
            })
        });
        if by_rule {
            return Some(Coverage::Rule);
        }
        let by_builtin = builtin.with(|b| {
            b.as_ref().is_some_and(|b| {
                let list = match scope {
                    RuleScope::Calendar => &b.calendar.blocked,
                    _ => &b.parcel.blocked,
                };
                list.iter().any(|p| matches(p))
            })
        });
        by_builtin.then_some(Coverage::Builtin)
    });

    let handle_reprocess = move |_| {
        if reprocessing.get_untracked() {
            return;
        }
        reprocessing.set(true);
        action_error.set(None);
        let id = activity_id.get_untracked();
        spawn_detached(async move {
            match api::reprocess_email_activity(&id).await {
                Ok(res) => {
                    current.set(res.activity.clone());
                    if let Some(cb) = on_activity_change {
                        cb.try_run(res.activity);
                    }
                    // The rerun may have resubmitted a forgotten number.
                    forgotten.set(HashSet::new());
                    logs_version.update(|v| *v += 1);
                }
                Err(e) => action_error.set(Some(e.message().to_owned())),
            }
            reprocessing.set(false);
        });
    };

    let handle_feedback = move |verdict: EmailFeedbackVerdict| {
        if fb_busy.get_untracked() {
            return;
        }
        let next = if fb.with_untracked(|f| f.as_ref().map(|f| f.verdict)) == Some(verdict) {
            None
        } else {
            Some(verdict)
        };
        fb_busy.set(true);
        action_error.set(None);
        let id = activity_id.get_untracked();
        spawn_detached(async move {
            match api::send_email_activity_feedback(&id, next, None).await {
                Ok(res) => {
                    fb.set(res.feedback.clone());
                    if let Some(cb) = on_feedback_change {
                        cb.try_run((id, res.feedback));
                    }
                }
                Err(e) => action_error.set(Some(e.message().to_owned())),
            }
            fb_busy.set(false);
        });
    };

    let handle_block = move |_| {
        if blocking.get_untracked() {
            return;
        }
        // Domain-only by default ("@doordash.com"); the toggle narrows to the address.
        let at_domain = sender_at_domain.get_untracked();
        let pattern = if block_exact.get_untracked() || at_domain.is_empty() {
            sender_email.get_untracked()
        } else {
            at_domain
        };
        let scope = block_scope_value.get_untracked();
        let scope_label = if scope == RuleScope::Both {
            "Parcels & Calendar"
        } else {
            pipeline_label(current.with_untracked(|a| a.pipeline))
        };
        if !confirm(&format!(
            "Block {pattern} for {scope_label}? Future emails from this sender will be filtered."
        )) {
            return;
        }
        blocking.set(true);
        action_error.set(None);
        spawn_detached(async move {
            let input = EmailRuleInput {
                pattern,
                scope,
                verdict: RuleVerdict::Block,
            };
            match api::create_email_rule(&input).await {
                Ok(res) => match res.status {
                    RuleUpsertStatus::Created => {
                        coverage_override.set(Some(Coverage::Rule));
                        toast.show("Sender blocked", ToastKind::Info);
                    }
                    RuleUpsertStatus::Merged => {
                        coverage_override.set(Some(Coverage::Rule));
                        toast.show(
                            res.message
                                .unwrap_or_else(|| "Merged into a single Both rule".into()),
                            ToastKind::Info,
                        );
                    }
                    RuleUpsertStatus::Exists => toast.show(
                        res.message
                            .unwrap_or_else(|| "Sender is already blocked".into()),
                        ToastKind::Error,
                    ),
                    RuleUpsertStatus::Builtin => {
                        coverage_override.set(Some(Coverage::Builtin));
                        toast.show(
                            res.message
                                .unwrap_or_else(|| "Already blocked by a built-in list".into()),
                            ToastKind::Info,
                        );
                    }
                },
                Err(e) => action_error.set(Some(e.message().to_owned())),
            }
            blocking.set(false);
        });
    };

    let handle_forget = move |tracking_number: String| {
        if forgetting.with_untracked(Option::is_some) {
            return;
        }
        if !confirm(&format!(
            "Forget tracking number {tracking_number}? A future email will be able to resubmit it."
        )) {
            return;
        }
        forgetting.set(Some(tracking_number.clone()));
        action_error.set(None);
        spawn_detached(async move {
            match api::forget_parcel_delivery(&tracking_number).await {
                Ok(_) => forgotten.update(|set| {
                    set.insert(tracking_number);
                }),
                Err(e) => action_error.set(Some(e.message().to_owned())),
            }
            forgetting.set(None);
        });
    };

    let header = move || {
        let a = current.get();
        let cost = a
            .cost_cents
            .filter(|c| *c > 0.0)
            .and_then(|c| format_cents(Some(c)));
        view! {
            <div class="log-modal-title">
                <span class=format!("mail-outcome mail-outcome-{}", a.outcome.as_str())>
                    {outcome_label(a.outcome)}
                </span>
                <span class="log-modal-task email-log-subject">
                    {if a.subject.is_empty() { "(no subject)".to_owned() } else { a.subject.clone() }}
                </span>
            </div>
            <div class="log-modal-meta meta-row muted">
                <span>{pipeline_label(a.pipeline)}</span>
                <span class="email-log-from">{a.from.clone()}</span>
                <span>{format_absolute(a.processed_at as f64)}</span>
                {a.admit_tier.map(|tier| {
                    view! {
                        <span class=format!("email-tier email-tier-{}", tier.as_str())>
                            {admit_tier_label(tier)}
                        </span>
                    }
                })}
                {cost.map(|cost| {
                    view! { <span class="email-cost" title="LLM cost for this email">{cost}</span> }
                })}
            </div>
        }
    };
    let details = move || {
        let a = current.get();
        view! {
            {a.detail.filter(|d| !d.is_empty()).map(|d| view! { <div class="muted log-modal-error">{d}</div> })}
            {a.admit_reason.filter(|r| !r.is_empty()).map(|r| {
                view! { <div class="muted email-admit-line">"Admitted: " {r}</div> }
            })}
        }
    };
    let items = move || {
        let a = current.get();
        (!a.items.is_empty()).then(|| {
            let is_parcel = a.pipeline == EmailPipelineName::ParcelTracker;
            let rows = a
                .items
                .into_iter()
                .map(|item| {
                    let tracking = if is_parcel { extract_tracking_number(&item) } else { None };
                    let row = move || {
                        let is_forgotten = tracking
                            .as_ref()
                            .is_some_and(|t| forgotten.with(|f| f.contains(t)));
                        let button = tracking.clone().filter(|_| !is_forgotten).map(|number| {
                            let label_number = number.clone();
                            view! {
                                <button
                                    type="button"
                                    class="email-item-forget"
                                    disabled=move || forgetting.with(Option::is_some)
                                    on:click=move |_| handle_forget(number.clone())
                                >
                                    {move || {
                                        if forgetting.get().as_ref() == Some(&label_number) {
                                            "Forgetting…"
                                        } else {
                                            "Forget"
                                        }
                                    }}
                                </button>
                            }
                        });
                        view! {
                            <span class=is_forgotten.then_some("email-item-forgotten")>{item.clone()}</span>
                            {is_forgotten.then(|| view! { <span class="email-item-note muted">"Forgotten"</span> })}
                            {button}
                        }
                    };
                    view! { <li>{row}</li> }
                })
                .collect_view();
            view! { <ul class="email-modal-items">{rows}</ul> }
        })
    };
    let feedback_buttons = move || {
        let p = pipeline();
        [
            EmailFeedbackVerdict::NotRelevant,
            EmailFeedbackVerdict::Missed,
        ]
        .into_iter()
        .map(|verdict| {
            view! {
                <button
                    type="button"
                    class=move || {
                        let active = fb.with(|f| f.as_ref().map(|f| f.verdict)) == Some(verdict);
                        format!("chip-btn {}", if active { "active" } else { "" })
                    }
                    disabled=move || fb_busy.get()
                    on:click=move |_| handle_feedback(verdict)
                >
                    {feedback_label(p, verdict)}
                </button>
            }
        })
        .collect_view()
    };
    let block_controls = move || match coverage.get() {
        Some(Coverage::Rule) => view! {
            <span class="email-rule-status email-rule-status-rule">"Sender Blocked"</span>
        }
        .into_any(),
        Some(Coverage::Builtin) => view! {
            <span class="email-rule-status email-rule-status-builtin">"Blocked by Built-in"</span>
        }
        .into_any(),
        None => {
            let p = pipeline();
            let scope = block_scope(p);
            view! {
                <select
                    class="email-block-scope"
                    prop:value=move || block_scope_value.get().as_str()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        block_scope_value.set(if value == "both" { RuleScope::Both } else { scope });
                    }
                    disabled=move || blocking.get()
                    aria-label="Block scope"
                >
                    <option value="both">"Both Pipelines"</option>
                    <option value=scope.as_str()>{format!("{} Only", pipeline_label(p))}</option>
                </select>
                <label class="email-block-exact">
                    <input
                        type="checkbox"
                        prop:checked=move || block_exact.get()
                        disabled=move || blocking.get()
                        on:change=move |ev| block_exact.set(event_target_checked(&ev))
                    />
                    "Exact Address Only"
                </label>
                <button
                    type="button"
                    class="email-block-btn"
                    disabled=move || blocking.get()
                    on:click=handle_block
                >
                    {move || if blocking.get() { "Blocking…" } else { "Block Sender" }}
                </button>
            }
            .into_any()
        }
    };
    let body = move || {
        if let Some(err) = error.get() {
            return view! { <div class="error-inline">"Failed to load logs: " {err}</div> }
                .into_any();
        }
        match lines.get() {
            None => view! { <div class="loading-inline">"Loading logs…"</div> }.into_any(),
            Some(l) if l.is_empty() => view! {
                <div class="muted log-empty">
                    "No processing logs for this email — it never reached extraction."
                </div>
            }
            .into_any(),
            Some(l) => view! { <LogLines lines=Signal::stored(l)/> }.into_any(),
        }
    };

    view! {
        <div class="modal-root">
            <button
                type="button"
                class="modal-backdrop"
                tabindex="-1"
                on:click=move |_| on_close.run(())
                aria-label="Close log viewer"
            ></button>
            <div
                class="log-modal"
                node_ref=modal_ref
                tabindex="-1"
                aria-modal="true"
                role="dialog"
                aria-label=move || {
                    let subject = current.with(|a| a.subject.clone());
                    format!("Logs for {}", if subject.is_empty() { "email".to_owned() } else { subject })
                }
            >
                <Toast toast=toast.toast/>
                <div class="log-modal-header">
                    {header}
                    <button
                        type="button"
                        class="log-modal-close"
                        on:click=move |_| on_close.run(())
                        aria-label="Close"
                    >
                        "✕"
                    </button>
                </div>
                {details}
                {items}
                <div class="email-actions">
                    <button
                        type="button"
                        class="run-btn"
                        disabled=move || reprocessing.get()
                        on:click=handle_reprocess
                    >
                        {move || {
                            reprocessing.get().then(|| view! { <span class="spinner" aria-hidden="true"></span> })
                        }}
                        {move || if reprocessing.get() { "Reprocessing…" } else { "Reprocess" }}
                    </button>
                    {feedback_buttons}
                    {block_controls}
                </div>
                {move || {
                    action_error
                        .get()
                        .map(|e| view! { <div class="email-action-error error-inline">{e}</div> })
                }}
                <div class="log-modal-body">{body}</div>
                {move || {
                    let dropped = dropped.get();
                    (dropped > 0)
                        .then(|| {
                            view! {
                                <div class="log-modal-footer muted">
                                    {format!("{dropped} oldest lines dropped")}
                                </div>
                            }
                        })
                }}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tracking_numbers_and_senders() {
        assert_eq!(
            extract_tracking_number("1Z999 (UPS): out for delivery").as_deref(),
            Some("1Z999")
        );
        assert_eq!(
            extract_tracking_number("ABC123: submitted").as_deref(),
            Some("ABC123")
        );
        assert_eq!(extract_tracking_number("A:B: x").as_deref(), Some("A"));
        assert_eq!(extract_tracking_number("no token here"), None);
        assert_eq!(
            extract_sender_email("DoorDash <Orders@DoorDash.com>"),
            "orders@doordash.com"
        );
        assert_eq!(
            extract_sender_email(" orders@doordash.com "),
            "orders@doordash.com"
        );
    }
}
