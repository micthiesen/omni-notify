//! The email inspector: processing logs plus actions (reprocess, feedback,
//! block sender, per-item parcel "forget"). Destructive actions use
//! [`ConfirmButton`] instead of `window.confirm()`.

use std::collections::HashSet;

use leptos::prelude::*;
use omni_api::email::{
    AdmitTier, BuiltinRules, EmailActivity, EmailActivityOutcome, EmailFeedback,
    EmailFeedbackVerdict, EmailPipelineName, EmailRule, EmailRuleInput, RuleScope,
    RuleUpsertStatus, RuleVerdict,
};
use omni_api::runs::RunLogLine;

use super::badges::{Status, Tag};
use super::button::{Button, ButtonSize, ButtonVariant, ConfirmButton};
use super::controls::Chip;
use super::icon::Icon;
use super::inspector::Inspector;
use super::log_viewer::LogLines;
use super::states::{ErrorState, InlineNote, SkeletonRows};
use super::toast::{ToastKind, use_toast};
use super::tone::{StatusKind, Tone};
use crate::api;
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

/// Status shape of an email outcome.
pub fn outcome_kind(outcome: EmailActivityOutcome) -> StatusKind {
    match outcome {
        EmailActivityOutcome::Processed => StatusKind::Ok,
        EmailActivityOutcome::Partial => StatusKind::Warn,
        EmailActivityOutcome::Failed | EmailActivityOutcome::Error => StatusKind::Fault,
        EmailActivityOutcome::Filtered
        | EmailActivityOutcome::Skipped
        | EmailActivityOutcome::NoMatches => StatusKind::Idle,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coverage {
    Rule,
    Builtin,
}

/// Drawer (or docked pane) for one email activity.
#[component]
pub fn EmailInspector(
    activity: EmailActivity,
    #[prop(into, optional)] docked: Signal<bool>,
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

    let handle_reprocess = move || {
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

    let handle_block = move || {
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

    let block_pattern = move || {
        let at_domain = sender_at_domain.get();
        if block_exact.get() || at_domain.is_empty() {
            sender_email.get()
        } else {
            at_domain
        }
    };
    let title = Signal::derive(move || {
        current.with(|a| {
            if a.subject.is_empty() {
                "(no subject)".to_owned()
            } else {
                a.subject.clone()
            }
        })
    });
    let status = ViewFn::from(move || {
        let a = current.get();
        view! {
            <Status kind=outcome_kind(a.outcome) label=outcome_label(a.outcome)/>
            <Tag>{pipeline_label(a.pipeline)}</Tag>
            {a.admit_tier.map(|tier| view! { <Tag title="Admitted by">{admit_tier_label(tier)}</Tag> })}
        }
    });
    let actions = ViewFn::from(move || {
        view! {
            <Button
                size=ButtonSize::Sm
                icon=Icon::Refresh
                busy=reprocessing
                on_click=Callback::new(move |_| handle_reprocess())
            >
                {move || if reprocessing.get() { "Reprocessing" } else { "Reprocess" }}
            </Button>
            {move || {
                let p = pipeline();
                [EmailFeedbackVerdict::NotRelevant, EmailFeedbackVerdict::Missed]
                    .into_iter()
                    .map(|verdict| view! {
                        <Chip
                            pressed=Signal::derive(move || fb.with(|f| f.as_ref().map(|f| f.verdict)) == Some(verdict))
                            on_click=Callback::new(move |()| handle_feedback(verdict))
                            title="Feedback for the triage model"
                        >
                            {feedback_label(p, verdict)}
                        </Chip>
                    })
                    .collect_view()
            }}
        }
    });
    let meta = move || {
        let a = current.get();
        let cost = a
            .cost_cents
            .filter(|c| *c > 0.0)
            .and_then(|c| format_cents(Some(c)));
        view! {
            <dl class="kv">
                <dt>"From"</dt>
                <dd class="truncate" title=a.from.clone()>{a.from.clone()}</dd>
                <dt>"Processed"</dt>
                <dd class="num">{format_absolute(a.processed_at as f64)}</dd>
                {cost.map(|cost| view! { <dt>"LLM cost"</dt><dd class="num">{cost}</dd> })}
                {a.admit_reason.filter(|r| !r.is_empty()).map(|r| view! { <dt>"Admitted"</dt><dd>{r}</dd> })}
            </dl>
            {a.detail.filter(|d| !d.is_empty()).map(|d| view! { <InlineNote tone=Tone::Warn>{d}</InlineNote> })}
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
                            let busy_number = number.clone();
                            view! {
                                <ConfirmButton
                                    label="Forget"
                                    confirm_label="Forget number"
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::Sm
                                    title="A future email will be able to resubmit it"
                                    busy=Signal::derive(move || forgetting.get().as_ref() == Some(&busy_number))
                                    disabled=Signal::derive(move || forgetting.with(Option::is_some))
                                    disabled_reason="Another number is being forgotten"
                                    on_confirm=Callback::new(move |()| handle_forget(number.clone()))
                                />
                            }
                        });
                        view! {
                            <span class=if is_forgotten { "row-main off" } else { "row-main" }>
                                <span class="row-title mono">{item.clone()}</span>
                            </span>
                            <span class="row-end">
                                {is_forgotten.then(|| view! { <span class="small dim">"Forgotten"</span> })}
                                {button}
                            </span>
                        }
                    };
                    view! { <div class="row dense">{row}</div> }
                })
                .collect_view();
            view! {
                <section class="inspector-section">
                    <h3 class="label">"Items"</h3>
                    <div class="rows">{rows}</div>
                </section>
            }
        })
    };
    let block_controls = move || match coverage.get() {
        Some(Coverage::Rule) => {
            view! { <Status kind=StatusKind::Ok label="Sender blocked"/> }.into_any()
        }
        Some(Coverage::Builtin) => {
            view! { <Status kind=StatusKind::Idle label="Blocked by a built-in list"/> }.into_any()
        }
        None => {
            let p = pipeline();
            let scope = block_scope(p);
            view! {
                <div class="toolbar">
                    <select
                        class="select"
                        prop:value=move || block_scope_value.get().as_str()
                        on:change=move |ev| {
                            let value = event_target_value(&ev);
                            block_scope_value.set(if value == "both" { RuleScope::Both } else { scope });
                        }
                        disabled=move || blocking.get()
                        aria-label="Block scope"
                    >
                        <option value="both">"Both pipelines"</option>
                        <option value=scope.as_str()>{format!("{} only", pipeline_label(p))}</option>
                    </select>
                    <label class="checkbox">
                        <input
                            type="checkbox"
                            prop:checked=move || block_exact.get()
                            disabled=move || blocking.get()
                            on:change=move |ev| block_exact.set(event_target_checked(&ev))
                        />
                        "Exact address only"
                    </label>
                </div>
                <ConfirmButton
                    label=Signal::derive(move || format!("Block {}", block_pattern()))
                    confirm_label="Confirm block"
                    variant=ButtonVariant::Danger
                    size=ButtonSize::Sm
                    busy=blocking
                    title="Future emails from this sender will be filtered"
                    on_confirm=Callback::new(move |()| handle_block())
                />
            }
            .into_any()
        }
    };
    let logs = move || {
        if let Some(err) = error.get() {
            return view! {
                <ErrorState
                    title="Could not load the processing log"
                    raw=err
                    retry=Callback::new(move |()| logs_version.update(|v| *v += 1))
                />
            }
            .into_any();
        }
        match lines.get() {
            None => view! { <SkeletonRows count=4 label="Loading log"/> }.into_any(),
            Some(l) if l.is_empty() => view! {
                <p class="log-note">"No processing log: this email never reached extraction."</p>
            }
            .into_any(),
            Some(l) => view! {
                <div class="logwell bounded" role="log">
                    {move || {
                        let dropped = dropped.get();
                        (dropped > 0).then(|| view! { <p class="log-note warn">{format!("{dropped} oldest lines were dropped.")}</p> })
                    }}
                    <LogLines lines=Signal::stored(l)/>
                </div>
            }
            .into_any(),
        }
    };

    view! {
        <Inspector title status actions on_close docked>
            {move || action_error.get().map(|e| view! { <ErrorState title="That action failed" detail=e/> })}
            <section class="inspector-section">{meta}</section>
            {items}
            <section class="inspector-section">
                <h3 class="label">"Sender"</h3>
                {block_controls}
            </section>
            <section class="inspector-section">
                <h3 class="label">"Processing log"</h3>
                {logs}
            </section>
        </Inspector>
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
