//! Briefing notification feed.

use leptos::prelude::*;
use omni_api::briefings::BriefingSummary;
use omni_api::runs::Run;
use omni_web_kit::api;
use omni_web_kit::components::{LogViewer, ShowMoreButton, use_show_more};
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute_with_year, format_cents, to_title_case};

use crate::common::active_if;

/// One notification with the briefing it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedEntry {
    pub briefing_name: String,
    pub title: String,
    pub message: String,
    pub url: String,
    pub timestamp: i64,
    pub run_id: Option<String>,
    pub cost_cents: Option<f64>,
}

/// Every notification of the selected briefing(s), newest first (stable).
pub fn build_feed(briefings: &[BriefingSummary], filter: Option<&str>) -> Vec<FeedEntry> {
    let mut feed: Vec<FeedEntry> = briefings
        .iter()
        .filter(|b| filter.is_none_or(|f| b.name == f))
        .flat_map(|b| {
            b.notifications.iter().map(|n| FeedEntry {
                briefing_name: b.name.clone(),
                title: n.title.clone(),
                message: n.message.clone(),
                url: n.url.clone(),
                timestamp: n.timestamp,
                run_id: n.run_id.clone(),
                cost_cents: n.cost_cents,
            })
        })
        .collect();
    feed.sort_by_key(|entry| std::cmp::Reverse(entry.timestamp));
    feed
}

#[component]
pub fn BriefingsPage() -> impl IntoView {
    let briefings = RwSignal::new(None::<Vec<BriefingSummary>>);
    let error = RwSignal::new(None::<String>);
    let filter = RwSignal::new(None::<String>);
    let log_run = RwSignal::new(None::<Run>);
    let logs_error = RwSignal::new(None::<String>);

    let open_logs = move |run_id: String| {
        logs_error.set(None);
        spawn_detached(async move {
            match api::fetch_run_logs(&run_id).await {
                Ok(logs) => log_run.set(Some(logs.run)),
                Err(err) => logs_error.set(Some(err.message().to_owned())),
            }
        });
    };

    spawn_scoped(async move {
        match api::fetch_briefings().await {
            Ok(res) => briefings.set(Some(res.briefings)),
            Err(err) => error.set(Some(err.message().to_owned())),
        }
    });

    let feed = Memo::new(move |_| {
        let filter = filter.get();
        briefings.with(|b| {
            b.as_deref()
                .map(|b| build_feed(b, filter.as_deref()))
                .unwrap_or_default()
        })
    });
    let show = use_show_more(
        feed.into(),
        20,
        Signal::derive(move || format!("{:?}", filter.get())),
    );

    let chips = move || {
        briefings
            .get()
            .unwrap_or_default()
            .into_iter()
            .map(|b| {
                let name = b.name.clone();
                let pressed_name = name.clone();
                let pressed = move || filter.with(|f| f.as_deref() == Some(pressed_name.as_str()));
                let class_pressed = pressed.clone();
                let aria_pressed = pressed.clone();
                view! {
                    <button
                        type="button"
                        class=move || format!("chip-btn {}", active_if(class_pressed()))
                        aria-pressed=move || aria_pressed().to_string()
                        on:click=move |_| {
                            filter.set(if pressed() { None } else { Some(name.clone()) })
                        }
                    >
                        {to_title_case(&b.name)}
                        <span class="chip-btn-count">{b.notifications.len()}</span>
                    </button>
                }
            })
            .collect_view()
    };

    let card = move |(_, entry): (usize, FeedEntry)| {
        let cost = format_cents(entry.cost_cents);
        let run_id = entry.run_id.clone().filter(|r| !r.is_empty());
        view! {
            <article class="briefing-card">
                <div class="briefing-card-header">
                    <h2 class="briefing-card-title">{entry.title.clone()}</h2>
                    <span class="briefing-card-time">
                        {format_absolute_with_year(entry.timestamp as f64)}
                    </span>
                </div>
                <p class="briefing-message">{entry.message.clone()}</p>
                <div class="briefing-card-meta">
                    <span class="briefing-badge">{to_title_case(&entry.briefing_name)}</span>
                    {(!entry.url.is_empty())
                        .then(|| {
                            view! {
                                <a href=entry.url.clone() target="_blank" rel="noreferrer" class="briefing-source">
                                    "Source ↗"
                                </a>
                            }
                        })}
                    {cost.map(|c| view! { <span class="briefing-cost">{c}</span> })}
                    {run_id
                        .map(|run_id| {
                            view! {
                                <button
                                    type="button"
                                    class="briefing-logs-btn"
                                    on:click=move |_| open_logs(run_id.clone())
                                >
                                    "Logs"
                                </button>
                            }
                        })}
                </div>
            </article>
        }
    };

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"Briefings"</h1>
                <p class="page-subtitle">"Updates worth knowing, collected from your briefings."</p>
            </div>
        </div>

        {move || {
            (briefings.with(Option::is_none) && error.with(Option::is_none))
                .then(|| view! { <div class="loading">"Loading…"</div> })
        }}
        {move || {
            error
                .get()
                .filter(|_| briefings.with(Option::is_none))
                .map(|e| {
                    view! {
                        <div class="error">
                            <div>"Failed to load briefings"</div>
                            <div class="error-detail">{e}</div>
                        </div>
                    }
                })
        }}
        {move || {
            briefings
                .with(|b| b.as_ref().is_some_and(Vec::is_empty))
                .then(|| view! { <div class="muted">"No briefings have run yet."</div> })
        }}
        {move || {
            briefings
                .with(|b| b.as_ref().is_some_and(|b| !b.is_empty()))
                .then(|| {
                    view! {
                        <div class="rec-filters" role="group" aria-label="Filter by Briefing">
                            <button
                                type="button"
                                class=move || format!("chip-btn {}", active_if(filter.with(Option::is_none)))
                                aria-pressed=move || filter.with(Option::is_none).to_string()
                                on:click=move |_| filter.set(None)
                            >
                                "All"
                            </button>
                            {chips}
                        </div>
                        <div class="briefing-feed">
                            <For
                                each=move || show.visible.get().into_iter().enumerate()
                                key=|(index, entry)| format!("{index}-{}-{}", entry.briefing_name, entry.timestamp)
                                children=card
                            />
                            {move || {
                                feed.with(Vec::is_empty)
                                    .then(|| view! { <div class="muted">"No notifications for this briefing yet."</div> })
                            }}
                        </div>
                        {move || {
                            show.has_more
                                .get()
                                .then(|| {
                                    view! {
                                        <ShowMoreButton
                                            remaining=show.remaining
                                            on_click=Callback::new(move |()| show.show_more())
                                        />
                                    }
                                })
                        }}
                    }
                })
        }}
        {move || logs_error.get().map(|e| view! { <div class="briefing-logs-error">{e}</div> })}
        {move || {
            log_run
                .get()
                .map(|run| view! { <LogViewer run=run on_close=Callback::new(move |()| log_run.set(None)) /> })
        }}
    }
}

#[cfg(test)]
mod tests {
    use omni_api::briefings::{BriefingNotification, BriefingSummary};

    use super::build_feed;

    fn note(title: &str, timestamp: i64) -> BriefingNotification {
        BriefingNotification {
            title: title.into(),
            message: String::new(),
            url: String::new(),
            timestamp,
            run_id: None,
            cost_cents: None,
        }
    }

    #[test]
    fn feed_merges_briefings_newest_first_and_filters() {
        let briefings = vec![
            BriefingSummary {
                name: "a".into(),
                notifications: vec![note("a2", 20), note("a1", 10)],
            },
            BriefingSummary {
                name: "b".into(),
                notifications: vec![note("b1", 15), note("b0", 10)],
            },
        ];
        let titles = |filter| {
            build_feed(&briefings, filter)
                .into_iter()
                .map(|e| e.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(None), ["a2", "b1", "a1", "b0"]);
        assert_eq!(titles(Some("b")), ["b1", "b0"]);
    }
}
