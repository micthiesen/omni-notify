//! Briefings: every notification the briefings sent, grouped by day, with
//! source chips, in-place expansion and run logs in the inspector.

use leptos::prelude::*;
use omni_api::briefings::BriefingSummary;
use omni_api::runs::Run;
use omni_web_kit::api;
use omni_web_kit::components::badges::status_str;
use omni_web_kit::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, Chip, EmptyState, ErrorState, Icon, Inspector,
    LogViewer, LogWell, PageHead, ShowMoreButton, SkeletonRows, Tag, use_show_more,
};
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{
    format_absolute_with_year, format_cents, format_duration, to_title_case,
};
use omni_web_kit::utils::js::{
    date_locale_date_string, date_locale_time_string, local_day_index, now_ms,
};

use crate::research_nav::{ResearchSwitch, ResearchTab};

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

/// Consecutive entries that share a local day: `(day index, entries)`.
pub fn group_by_day(
    entries: &[FeedEntry],
    day_of: impl Fn(f64) -> i64,
) -> Vec<(i64, Vec<FeedEntry>)> {
    let mut groups: Vec<(i64, Vec<FeedEntry>)> = Vec::new();
    for entry in entries {
        let day = day_of(entry.timestamp as f64);
        match groups.last_mut() {
            Some((last, items)) if *last == day => items.push(entry.clone()),
            _ => groups.push((day, vec![entry.clone()])),
        }
    }
    groups
}

/// "Today", "Yesterday" or "Mon, Oct 6".
fn day_label(day: i64, today: i64, sample_ms: f64) -> String {
    match today - day {
        0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        _ => date_locale_date_string(
            sample_ms,
            &[("weekday", "short"), ("month", "short"), ("day", "numeric")],
        ),
    }
}

fn time_of_day(ms: f64) -> String {
    date_locale_time_string(ms, &[("hour", "numeric"), ("minute", "2-digit")])
}

#[component]
pub fn BriefingsPage() -> impl IntoView {
    let briefings = RwSignal::new(None::<Vec<BriefingSummary>>);
    let error = RwSignal::new(None::<String>);
    let filter = RwSignal::new(None::<String>);
    let log_run = RwSignal::new(None::<Run>);
    let expanded = RwSignal::new(false);
    let loading_logs = RwSignal::new(None::<String>);
    let logs_error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);

    Effect::new(move |_| {
        reload.track();
        error.set(None);
        spawn_scoped(async move {
            match api::fetch_briefings().await {
                Ok(res) => briefings.set(Some(res.briefings)),
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    });

    let open_logs = move |run_id: String| {
        logs_error.set(None);
        loading_logs.set(Some(run_id.clone()));
        spawn_detached(async move {
            match api::fetch_run_logs(&run_id).await {
                Ok(logs) => {
                    expanded.try_set(false);
                    log_run.try_set(Some(logs.run));
                }
                Err(err) => {
                    logs_error.try_set(Some(err.message().to_owned()));
                }
            }
            loading_logs.try_set(None);
        });
    };

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
        30,
        Signal::derive(move || format!("{:?}", filter.get())),
    );
    let total = Memo::new(move |_| {
        briefings.with(|b| {
            b.as_ref().map_or(0, |b| {
                b.iter().map(|b| b.notifications.len()).sum::<usize>()
            })
        })
    });
    let lede = Signal::derive(move || {
        let today = local_day_index(now_ms());
        let (count, spent) = briefings.with(|b| {
            let mut count = 0;
            let mut spent = 0.0;
            for n in b.iter().flatten().flat_map(|b| b.notifications.iter()) {
                if local_day_index(n.timestamp as f64) == today {
                    count += 1;
                }
                spent += n.cost_cents.unwrap_or(0.0);
            }
            (count, spent)
        });
        let sources = briefings.with(|b| b.as_ref().map_or(0, Vec::len));
        Some(format!(
            "{} today · {} updates from {} briefings{}",
            if count == 0 {
                "Nothing new".to_owned()
            } else {
                format!("{count} new")
            },
            total.get(),
            sources,
            format_cents(Some(spent))
                .filter(|_| spent > 0.0)
                .map(|c| format!(" · {c} spent"))
                .unwrap_or_default(),
        ))
    });

    let chips = move || {
        let mut list = briefings.get().unwrap_or_default();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list.into_iter()
            .map(|b| {
                let name = b.name.clone();
                let pressed_name = name.clone();
                view! {
                    <Chip
                        pressed=Signal::derive(move || filter.with(|f| f.as_deref() == Some(pressed_name.as_str())))
                        count=b.notifications.len()
                        on_click=Callback::new(move |()| {
                            let next = name.clone();
                            filter.update(|f| *f = if f.as_deref() == Some(next.as_str()) { None } else { Some(next) });
                        })
                    >
                        {to_title_case(&b.name)}
                    </Chip>
                }
            })
            .collect_view()
    };

    let entry_row = move |entry: FeedEntry| {
        let cost = format_cents(entry.cost_cents);
        let run_id = entry.run_id.clone().filter(|r| !r.is_empty());
        let at = entry.timestamp as f64;
        let has_url = !entry.url.is_empty();
        let busy_id = run_id.clone();
        view! {
            <details class="brief">
                <summary class="brief-row">
                    <span class="brief-time mono small muted" title=format_absolute_with_year(at)>{time_of_day(at)}</span>
                    <span class="brief-main">
                        <span class="brief-title">{entry.title.clone()}</span>
                        <span class="brief-preview small muted">{entry.message.clone()}</span>
                    </span>
                    <span class="brief-end">
                        <Tag>{to_title_case(&entry.briefing_name)}</Tag>
                        {cost.clone().map(|c| view! { <span class="mono small muted hide-phone">{c}</span> })}
                    </span>
                </summary>
                <div class="brief-body">
                    <p class="prose brief-message">{entry.message.clone()}</p>
                    <div class="cluster">
                        {has_url
                            .then(|| {
                                view! {
                                    <ButtonLink to=entry.url.clone() external=true size=ButtonSize::Sm>
                                        "Open source"
                                    </ButtonLink>
                                }
                            })}
                        {run_id
                            .map(|run_id| {
                                let busy = Signal::derive(move || loading_logs.get() == busy_id);
                                view! {
                                    <Button
                                        size=ButtonSize::Sm
                                        variant=ButtonVariant::Ghost
                                        icon=Icon::Logs
                                        busy
                                        on_click=Callback::new(move |_| open_logs(run_id.clone()))
                                    >
                                        "Logs"
                                    </Button>
                                }
                            })}
                        {cost.map(|c| view! { <span class="mono small muted only-phone">{c}</span> })}
                    </div>
                </div>
            </details>
        }
    };

    let groups = move || {
        let today = local_day_index(now_ms());
        let visible = show.visible.get();
        group_by_day(&visible, local_day_index)
            .into_iter()
            .map(|(day, entries)| {
                let label = day_label(day, today, entries[0].timestamp as f64);
                let count = entries.len();
                view! {
                    <div class="group-head">
                        {label}
                        <span class="seg-n">{count}</span>
                    </div>
                    <div class="rows brief-list">
                        {entries.into_iter().map(entry_row).collect_view()}
                    </div>
                }
            })
            .collect_view()
    };

    let inspector = move || {
        let run = log_run.get()?;
        let title = format!("{} run", to_title_case(&run.task_name));
        let started = run.started_at as f64;
        let duration = run
            .finished_at
            .map(|f| format_duration((f - run.started_at) as f64));
        let status = status_str(run.status);
        let well_run = run.clone();
        let full_run = run.clone();
        Some(view! {
            <Inspector
                title=title
                on_close=Callback::new(move |()| log_run.set(None))
                actions=ViewFn::from(move || {
                    view! {
                        <Button size=ButtonSize::Sm icon=Icon::Logs on_click=Callback::new(move |_| expanded.set(true))>
                            "Expand"
                        </Button>
                    }
                })
            >
                <section class="inspector-section">
                    <dl class="kv">
                        <dt>"Started"</dt>
                        <dd class="num">{format_absolute_with_year(started)}</dd>
                        <dt>"Status"</dt>
                        <dd>{status}</dd>
                        {duration.clone().map(|d| view! { <dt>"Duration"</dt><dd class="num">{d}</dd> })}
                    </dl>
                </section>
                <section class="inspector-section">
                    <LogWell run=well_run.clone() />
                </section>
            </Inspector>
            {move || {
                expanded
                    .get()
                    .then(|| view! { <LogViewer run=full_run.clone() on_close=Callback::new(move |()| expanded.set(false)) /> })
            }}
        })
    };

    view! {
        <PageHead title="Briefings" lede />
        <ResearchSwitch current=ResearchTab::Briefings />
        {move || {
            if briefings.with(Option::is_none) {
                return match error.get() {
                    Some(e) => view! {
                        <ErrorState
                            title="Briefings could not load"
                            raw=e
                            retry=Callback::new(move |()| reload.update(|n| *n += 1))
                            page=true
                        />
                    }
                    .into_any(),
                    None => view! { <SkeletonRows count=8 label="Loading briefings" /> }.into_any(),
                };
            }
            if total.get() == 0 {
                return view! {
                    <EmptyState
                        message="No briefings yet. They appear here after their first run."
                        icon=Icon::Doc
                    />
                }
                .into_any();
            }
            view! {
                <div class="chips brief-chips" role="group" aria-label="Filter by briefing">
                    <Chip
                        pressed=Signal::derive(move || filter.with(Option::is_none))
                        count=total
                        on_click=Callback::new(move |()| filter.set(None))
                    >
                        "All"
                    </Chip>
                    {chips}
                </div>
                {move || logs_error.get().map(|e| view! { <ErrorState title="Those logs could not load" raw=e /> })}
                <div class="panel brief-feed">
                    {move || {
                        if feed.with(Vec::is_empty) {
                            view! {
                                <EmptyState
                                    compact=true
                                    message="No notifications from this briefing yet."
                                    action=ViewFn::from(move || view! {
                                        <Button size=ButtonSize::Sm on_click=Callback::new(move |_| filter.set(None))>"Show all"</Button>
                                    })
                                />
                            }
                                .into_any()
                        } else {
                            groups().into_any()
                        }
                    }}
                </div>
                {move || {
                    show.has_more
                        .get()
                        .then(|| {
                            view! {
                                <ShowMoreButton
                                    remaining=show.remaining
                                    noun="updates"
                                    on_click=Callback::new(move |()| show.show_more())
                                />
                            }
                        })
                }}
            }
            .into_any()
        }}
        {inspector}
    }
}

#[cfg(test)]
mod tests {
    use omni_api::briefings::{BriefingNotification, BriefingSummary};

    use super::{build_feed, group_by_day};

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

    #[test]
    fn feed_groups_consecutive_days() {
        let briefings = vec![BriefingSummary {
            name: "a".into(),
            notifications: vec![note("x", 250), note("y", 240), note("z", 120), note("w", 5)],
        }];
        let feed = build_feed(&briefings, None);
        let groups = group_by_day(&feed, |ms| (ms / 100.0).floor() as i64);
        let shape: Vec<(i64, usize)> = groups.iter().map(|(d, e)| (*d, e.len())).collect();
        assert_eq!(shape, [(2, 2), (1, 1), (0, 1)]);
    }
}
