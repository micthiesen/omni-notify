//! Small pieces shared by the recommendation and episode pages.

use leptos::prelude::*;
use omni_web_kit::components::{ToastHandle, ToastKind};
use omni_web_kit::live::LiveData;
use omni_web_kit::task::spawn_detached;
use omni_web_kit::utils::format::format_absolute_with_year;
use omni_web_kit::utils::js::{js_round, number_string};

/// `formatRuntime` / `formatEpisodeDuration`: `1h 5m`, `2h`, `45m`.
pub fn format_minutes(minutes: f64) -> String {
    let h = (minutes / 60.0).floor();
    let m = js_round(minutes % 60.0);
    if h > 0.0 {
        if m > 0.0 {
            format!("{}h {}m", number_string(h), number_string(m))
        } else {
            format!("{}h", number_string(h))
        }
    } else {
        format!("{}m", number_string(m))
    }
}

/// `"active"` when `on`, else empty (TS template-literal class toggles).
pub fn active_if(on: bool) -> &'static str {
    if on { "active" } else { "" }
}

#[component]
pub fn DetailField(#[prop(into)] label: String, children: Children) -> impl IntoView {
    view! {
        <div class="detail-field">
            <dt>{label}</dt>
            <dd>{children()}</dd>
        </div>
    }
}

#[component]
pub fn ScoreRow(label: &'static str, value: f64) -> impl IntoView {
    let width = format!("width: {}%;", number_string(value.clamp(0.0, 100.0)));
    view! {
        <div class="score-row">
            <span class="score-label">{label}</span>
            <span class="score-bar">
                <span class="score-bar-fill" style=width></span>
            </span>
            <span class="score-value">{number_string(js_round(value))}</span>
        </div>
    }
}

#[component]
pub fn TimelineRow(#[prop(into)] label: String, at: f64) -> impl IntoView {
    view! {
        <div class="timeline-row">
            <span class="timeline-label">{label}</span>
            <span class="timeline-time">{format_absolute_with_year(at)}</span>
        </div>
    }
}

/// `← <label>` back link above detail pages.
#[component]
pub fn BackLink(to: &'static str, label: &'static str) -> impl IntoView {
    view! {
        <omni_web_kit::router::Link to=to class="detail-back">
            <span class="detail-back-arrow" aria-hidden="true">"←"</span>
            {label}
        </omni_web_kit::router::Link>
    }
}

/// The "Picks" limit select and run button of the recommendation pages.
#[component]
pub fn RunControls(
    task_name: &'static str,
    select_label: &'static str,
    max_options: u32,
    disabled_title: &'static str,
    #[prop(into)] running: Signal<bool>,
    #[prop(into)] available: Signal<bool>,
    live: LiveData,
    toast: ToastHandle,
) -> impl IntoView {
    let max = RwSignal::new(1u32);
    let disabled = move || running.get() || !available.get();
    let on_run = move |_| {
        let picks = max.get_untracked();
        spawn_detached(async move {
            let result = live.run_task(task_name.to_owned(), Some(picks)).await;
            let kind = if result.ok {
                ToastKind::Info
            } else {
                ToastKind::Error
            };
            toast.show(result.message, kind);
        });
    };
    let options = (1..=max_options)
        .map(|count| {
            view! {
                <option value=count.to_string() prop:selected=move || max.get() == count>
                    {count}
                </option>
            }
        })
        .collect_view();
    view! {
        <div class="rec-run-controls">
            <label class="rec-run-limit">
                <span>"Picks"</span>
                <select
                    aria-label=select_label
                    disabled=disabled
                    on:change=move |event| {
                        max.set(event_target_value(&event).parse().unwrap_or(1));
                    }
                >
                    {options}
                </select>
            </label>
            <button
                type="button"
                class="run-btn"
                disabled=disabled
                title=move || (!available.get()).then_some(disabled_title)
                on:click=on_run
            >
                {move || {
                    if running.get() {
                        view! {
                            <span class="running-pulse"></span>
                            " Running…"
                        }
                            .into_any()
                    } else if available.get() {
                        "Find Picks".into_any()
                    } else {
                        "Task Disabled".into_any()
                    }
                }}
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::format_minutes;

    #[test]
    fn minutes_format_like_the_ts_helpers() {
        assert_eq!(format_minutes(45.0), "45m");
        assert_eq!(format_minutes(60.0), "1h");
        assert_eq!(format_minutes(125.0), "2h 5m");
        assert_eq!(format_minutes(0.0), "0m");
        // `Math.round(59.6 % 60)` is 60, which the TS helper prints as is.
        assert_eq!(format_minutes(59.6), "60m");
    }
}
