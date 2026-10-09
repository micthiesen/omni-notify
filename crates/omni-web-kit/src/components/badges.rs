//! Static labels: [`Tag`], [`Status`], [`LiveTag`], [`Delta`], [`CountBadge`]
//! and the run-specific [`TriggerBadge`] / [`StatusDot`].

use leptos::prelude::*;
use omni_api::runs::{RunStatus, RunTrigger};

use super::tone::{StatusKind, Tone};
use crate::utils::js::{js_round, number_string, to_fixed};

pub fn trigger_str(trigger: RunTrigger) -> &'static str {
    match trigger {
        RunTrigger::Schedule => "schedule",
        RunTrigger::Manual => "manual",
        RunTrigger::Startup => "startup",
        RunTrigger::Catchup => "catchup",
    }
}

pub fn trigger_label(trigger: RunTrigger) -> &'static str {
    match trigger {
        RunTrigger::Catchup => "catch-up",
        other => trigger_str(other),
    }
}

pub fn status_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Success => "success",
        RunStatus::Error => "error",
        RunStatus::Degraded => "degraded",
    }
}

/// The [`StatusKind`] of a run.
pub fn run_status_kind(status: RunStatus) -> StatusKind {
    match status {
        RunStatus::Running => StatusKind::Running,
        RunStatus::Success => StatusKind::Ok,
        RunStatus::Error => StatusKind::Fault,
        RunStatus::Degraded => StatusKind::Warn,
    }
}

/// Static mono label (tier, kind, policy, trigger).
#[component]
pub fn Tag(
    #[prop(optional)] tone: Tone,
    #[prop(into, optional)] title: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    let class = match tone.class() {
        "" => "tag".to_owned(),
        tone => format!("tag {tone}"),
    };
    view! { <span class=class title=move || title.get()>{children()}</span> }
}

/// Shape plus word. `label` overrides the kind's default word; `dot_only`
/// keeps the word for screen readers only (use for healthy rows).
#[component]
pub fn Status(
    #[prop(into)] kind: Signal<StatusKind>,
    #[prop(into, optional)] label: MaybeProp<String>,
    #[prop(optional)] dot_only: bool,
    #[prop(into, optional)] title: MaybeProp<String>,
) -> impl IntoView {
    let word = move || label.get().unwrap_or_else(|| kind.get().word().to_owned());
    view! {
        <span
            class=move || format!("status {}{}", kind.get().class(), if dot_only { " dot-only" } else { "" })
            title=move || title.get().or_else(|| dot_only.then(word))
        >
            {move || {
                if dot_only {
                    view! { <span class="sr-only">{word()}</span> }.into_any()
                } else {
                    view! { <span>{word()}</span> }.into_any()
                }
            }}
        </span>
    }
}

/// `LIVE` or `LIVE · 4h 12m`; render it only while live.
#[component]
pub fn LiveTag(#[prop(into, optional)] detail: MaybeProp<String>) -> impl IntoView {
    view! {
        <span class="live-tag">
            <span class="live-dot" aria-hidden="true"></span>
            "LIVE"
            {move || detail.get().map(|d| view! { <span class="num">{format!("· {d}")}</span> })}
        </span>
    }
}

/// Direction of a [`Delta`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeltaDirection {
    Up,
    Down,
    Flat,
}

/// Signed percent formatted as `↑8.1%` / `↓2.3%` / `±0%`.
pub fn delta_parts(percent: f64) -> (DeltaDirection, String) {
    let rounded = js_round(percent * 10.0) / 10.0;
    if rounded > 0.0 {
        (DeltaDirection::Up, format!("↑{}%", to_fixed(rounded, 1)))
    } else if rounded < 0.0 {
        (DeltaDirection::Down, format!("↓{}%", to_fixed(-rounded, 1)))
    } else {
        (DeltaDirection::Flat, "±0%".to_owned())
    }
}

/// Change readout. A dip is not an error (neutral text); `surge` shows the
/// backend's surge gate as a Signal word. The UI never computes a surge.
#[component]
pub fn Delta(
    #[prop(into)] percent: Signal<Option<f64>>,
    #[prop(into, optional)] surge: Signal<bool>,
    #[prop(into, optional)] title: MaybeProp<String>,
) -> impl IntoView {
    move || {
        if surge.get() {
            return Some(
                view! { <span class="delta surge" title=move || title.get()>"Surging"</span> }
                    .into_any(),
            );
        }
        percent.get().filter(|p| p.is_finite()).map(|p| {
            let (direction, text) = delta_parts(p);
            let class = match direction {
                DeltaDirection::Up => "delta up",
                DeltaDirection::Down => "delta down",
                DeltaDirection::Flat => "delta",
            };
            view! { <span class=class title=move || title.get()>{text}</span> }.into_any()
        })
    }
}

/// Small pill count (tab bar and rail badges). Zero renders nothing.
#[component]
pub fn CountBadge(
    #[prop(into)] count: Signal<usize>,
    #[prop(optional)] tone: Tone,
    #[prop(into, optional)] label: MaybeProp<String>,
) -> impl IntoView {
    move || {
        let n = count.get();
        (n > 0).then(|| {
            let text = if n > 99 {
                "99+".to_owned()
            } else {
                number_string(n as f64)
            };
            view! {
                <span class=format!("badge {}", tone.class()) aria-label=move || label.get()>
                    {text}
                </span>
            }
        })
    }
}

#[component]
pub fn TriggerBadge(trigger: RunTrigger) -> impl IntoView {
    let tone = if trigger == RunTrigger::Manual {
        Tone::Info
    } else {
        Tone::Neutral
    };
    view! { <Tag tone>{trigger_label(trigger)}</Tag> }
}

/// A run's status as a [`Status`].
#[component]
pub fn StatusDot(status: RunStatus) -> impl IntoView {
    let kind = run_status_kind(status);
    view! { <Status kind dot_only=kind == StatusKind::Ok/> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_round_to_one_decimal() {
        assert_eq!(delta_parts(8.14), (DeltaDirection::Up, "↑8.1%".into()));
        assert_eq!(delta_parts(-2.26), (DeltaDirection::Down, "↓2.3%".into()));
        assert_eq!(delta_parts(0.01), (DeltaDirection::Flat, "±0%".into()));
    }
}
