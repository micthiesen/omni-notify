//! One monitored streamer: live header, intelligence, viewer records, chart
//! and recent sessions (`pages/StreamerPage.tsx`).

use std::collections::HashMap;

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::intelligence::{FeedbackVerdict, LivestreamIntelligence, PresenceState};
use omni_api::streamers::{StreamSessionView, StreamerMetricsResponse, StreamerView};
use omni_web_kit::api;
use omni_web_kit::charts::{AxisStyle, BarChart, BarPoint, BarSeries};
use omni_web_kit::components::live_now::streamer_intelligence;
use omni_web_kit::components::{DggPresenceTag, PlatformIcon, ShowMoreButton, use_show_more};
use omni_web_kit::hooks::use_now;
use omni_web_kit::router::Link;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::use_live_data;
use omni_web_kit::utils::format::{
    format_compact_number, format_date_only, format_duration, format_relative, format_uptime,
};
use omni_web_kit::utils::js::{
    civil_from_days, date_locale_date_string, date_locale_time_string, days_from_civil, js_round,
    local_date_ms, locale_number, now_ms, number_string,
};

const MAX_SESSION_ROWS: usize = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Range {
    Days30,
    Days90,
    All,
}

impl Range {
    fn days(self) -> Option<i64> {
        match self {
            Range::Days30 => Some(30),
            Range::Days90 => Some(90),
            Range::All => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Range::Days30 => "30d",
            Range::Days90 => "90d",
            Range::All => "All",
        }
    }
}

/// UTC `YYYY-MM-DD` of day index `days` since the epoch.
fn date_string(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_date(date: &str) -> Option<(i64, i64, i64)> {
    let mut parts = date.split('-').map(|p| p.parse::<i64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// `d.setDate(d.getDate() - days); d.toISOString().slice(0, 10)`: shift the
/// local calendar date, then take the UTC date.
fn days_ago(days: i64) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::new_0();
        let shifted = js_sys::Date::new_with_year_month_day_hr_min_sec_milli(
            now.get_full_year(),
            now.get_month() as i32,
            now.get_date() as i32 - days as i32,
            now.get_hours() as i32,
            now.get_minutes() as i32,
            now.get_seconds() as i32,
            now.get_milliseconds() as i32,
        );
        String::from(shifted.to_iso_string())
            .chars()
            .take(10)
            .collect()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        date_string((now_ms() / 86_400_000.0).floor() as i64 - days)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct DayPoint {
    date: String,
    max_viewers: i64,
    streamed: bool,
}

/// Sparse daily buckets expanded into a continuous series so off days show
/// as gaps rather than being compressed away.
fn build_day_series(metrics: &StreamerMetricsResponse, range: Range) -> Vec<DayPoint> {
    let by_date: HashMap<&str, i64> = metrics
        .daily_buckets
        .iter()
        .map(|b| (b.date.as_str(), b.max_viewers))
        .collect();
    let Some(first_bucket) = metrics.daily_buckets.first().map(|b| b.date.clone()) else {
        return Vec::new();
    };
    let range_start = match range.days() {
        None => first_bucket.clone(),
        Some(days) => days_ago(days - 1),
    };
    // Never start before tracking began.
    let start = if range_start < first_bucket {
        first_bucket
    } else {
        range_start
    };
    let today = date_string((now_ms() / 86_400_000.0).floor() as i64);
    let Some((y, m, d)) = parse_date(&start) else {
        return Vec::new();
    };
    let mut cursor = days_from_civil(y, m, d);
    let mut series = Vec::new();
    loop {
        let date = date_string(cursor);
        if date > today {
            break;
        }
        let max = by_date.get(date.as_str()).copied();
        series.push(DayPoint {
            date,
            max_viewers: max.unwrap_or(0),
            streamed: max.is_some(),
        });
        cursor += 1;
    }
    series
}

fn local_day_ms(date: &str) -> f64 {
    parse_date(date).map_or(f64::NAN, |(y, m, d)| {
        local_date_ms(y as i32, m as i32, d as i32)
    })
}

fn format_day_tick(date: &str) -> String {
    date_locale_date_string(
        local_day_ms(date),
        &[("month", "short"), ("day", "numeric")],
    )
}

fn format_day_full(date: &str) -> String {
    date_locale_date_string(
        local_day_ms(date),
        &[
            ("weekday", "short"),
            ("month", "short"),
            ("day", "numeric"),
            ("year", "numeric"),
        ],
    )
}

fn window_max(metrics: &StreamerMetricsResponse, days: i64) -> i64 {
    let cutoff = days_ago(days);
    metrics
        .daily_buckets
        .iter()
        .filter(|b| b.date >= cutoff)
        .map(|b| b.max_viewers)
        .fold(0, i64::max)
}

#[component]
fn StreamerStats(metrics: StreamerMetricsResponse) -> impl IntoView {
    let cutoff = days_ago(30);
    let days_streamed = metrics
        .daily_buckets
        .iter()
        .filter(|b| b.date >= cutoff)
        .count();
    let highs: Vec<(&str, i64, Option<String>)> = vec![
        ("7-Day High", window_max(&metrics, 7), None),
        ("30-Day High", window_max(&metrics, 30), None),
        ("90-Day High", window_max(&metrics, 90), None),
        (
            "All-Time Record",
            metrics.all_time_max,
            (metrics.all_time_max_timestamp != 0)
                .then(|| format_date_only(metrics.all_time_max_timestamp as f64)),
        ),
    ];
    view! {
        <div class="stat-strip">
            {highs
                .into_iter()
                .map(|(label, value, detail)| view! {
                    <div class="stat-tile">
                        <span class="stat-label">{label}</span>
                        <span class="stat-value">
                            {if value > 0 { format_compact_number(value as f64) } else { "—".to_owned() }}
                        </span>
                        {detail.map(|d| view! { <span class="stat-detail">{d}</span> })}
                    </div>
                })
                .collect_view()}
            <div class="stat-tile">
                <span class="stat-label">"Days Streamed"</span>
                <span class="stat-value">{days_streamed}</span>
                <span class="stat-detail">"of last 30"</span>
            </div>
        </div>
    }
}

#[component]
fn PlatformStats(metrics: StreamerMetricsResponse) -> impl IntoView {
    (!metrics.platforms.is_empty()).then(|| view! {
        <section class="page-section">
            <h2 class="section-title">"Platform Records"</h2>
            <div class="stat-strip">
                {metrics
                    .platforms
                    .into_iter()
                    .map(|source| view! {
                        <div class="stat-tile">
                            <span class="stat-label">
                                <PlatformIcon platform=source.platform.clone() size=13/>
                                {format!(" {}", source.platform)}
                            </span>
                            <span class="stat-value">
                                {if source.all_time_max > 0 {
                                    format_compact_number(source.all_time_max as f64)
                                } else {
                                    "—".to_owned()
                                }}
                            </span>
                            <span class="stat-detail">{format!("{} all-time", source.username)}</span>
                        </div>
                    })
                    .collect_view()}
            </div>
        </section>
    })
}

#[component]
fn ViewerChart(metrics: StreamerMetricsResponse) -> impl IntoView {
    let range = RwSignal::new(Range::Days30);
    let series = Memo::new(move |_| build_day_series(&metrics, range.get()));
    let buttons = [Range::Days30, Range::Days90, Range::All]
        .into_iter()
        .map(|r| view! {
            <button
                type="button"
                class=move || format!("range-btn {}", if range.get() == r { "active" } else { "" })
                on:click=move |_| range.set(r)
            >
                {r.label()}
            </button>
        })
        .collect_view();
    let chart = move || {
        let points = series.get();
        if points.iter().all(|p| !p.streamed) {
            return view! { <div class="no-data">"No streams in this range"</div> }.into_any();
        }
        let bars: Vec<BarPoint> = points
            .iter()
            .map(|p| BarPoint {
                x: p.date.clone(),
                values: vec![p.max_viewers as f64],
            })
            .collect();
        let tooltip = Callback::new(move |index: usize| {
            let Some(point) = points.get(index) else {
                return ().into_any();
            };
            let row = if point.streamed {
                format!("{} peak viewers", locale_number(point.max_viewers as f64))
            } else {
                "No stream".to_owned()
            };
            view! {
                <div class="custom-tooltip">
                    <div class="tooltip-label">{format_day_full(&point.date)}</div>
                    <div class="tooltip-row">{row}</div>
                </div>
            }
            .into_any()
        });
        view! {
            <div class="chart-container">
                <BarChart
                    data=Signal::stored(bars)
                    series=Signal::stored(vec![BarSeries { key: "maxViewers".into(), color: "#38bdf8".into() }])
                    x_tick=Callback::new(|date: String| format_day_tick(&date))
                    y_tick=Callback::new(format_compact_number)
                    tooltip
                    y_width=44.0
                    max_bar_size=28.0
                    radius=3.0
                    cursor_fill="rgba(56, 189, 248, 0.08)"
                    axis=AxisStyle { tick: "#8888a8".into(), line: "#3a3a5a".into() }
                />
            </div>
        }
        .into_any()
    };
    view! {
        <section class="page-section">
            <h2 class="section-title">
                "Peak Viewers by Day"
                <span class="range-buttons">{buttons}</span>
            </h2>
            {chart}
        </section>
    }
}

fn format_session_date(timestamp: i64) -> String {
    date_locale_date_string(
        timestamp as f64,
        &[("weekday", "short"), ("month", "short"), ("day", "numeric")],
    )
}

fn format_session_time(timestamp: i64) -> String {
    date_locale_time_string(
        timestamp as f64,
        &[("hour", "numeric"), ("minute", "2-digit")],
    )
}

#[component]
fn RecentSessions(sessions: Vec<StreamSessionView>) -> impl IntoView {
    let count = sessions.len();
    let shown = use_show_more(
        Signal::stored(sessions),
        MAX_SESSION_ROWS,
        Signal::stored(String::new()),
    );
    view! {
        <section class="page-section">
            <h2 class="section-title">
                "Recent Streams"
                <span class="section-count">{count}</span>
            </h2>
            <ul class="session-list">
                {move || {
                    shown
                        .visible
                        .get()
                        .into_iter()
                        .map(|session| view! {
                            <li class="session-row">
                                <div class="session-when">
                                    <span class="session-date">{format_session_date(session.started_at)}</span>
                                    <span class="session-time">
                                        {format!(
                                            "{} – {}",
                                            format_session_time(session.started_at),
                                            format_session_time(session.ended_at),
                                        )}
                                    </span>
                                </div>
                                <div class="session-title" title=session.title.clone()>
                                    <PlatformIcon platform=session.platform.clone() size=13/>
                                    <span>
                                        {if session.title.is_empty() { "Untitled stream".to_owned() } else { session.title.clone() }}
                                    </span>
                                </div>
                                <div class="session-stats">
                                    <span>{format_duration(session.duration_ms as f64)}</span>
                                    <span class="session-peak">
                                        {if session.peak_viewers > 0 {
                                            format!("{} peak", format_compact_number(session.peak_viewers as f64))
                                        } else {
                                            "—".to_owned()
                                        }}
                                    </span>
                                </div>
                            </li>
                        })
                        .collect_view()
                }}
            </ul>
            {move || shown.has_more.get().then(|| view! {
                <ShowMoreButton remaining=shown.remaining on_click=Callback::new(move |()| shown.show_more())/>
            })}
        </section>
    }
}

#[component]
fn StreamerHeader(#[prop(into)] streamer: Signal<StreamerView>) -> impl IntoView {
    let now = use_now(1000);
    let title = move || match streamer.get() {
        StreamerView::Live(live) => view! {
            {live.display_name.clone()}
            <span class="live-badge">
                <span class="live-badge-dot"></span>
                "LIVE"
            </span>
            <DggPresenceTag dgg=live.dgg/>
        }
        .into_any(),
        StreamerView::Offline(offline) => view! {
            {offline.display_name.clone()}
            <DggPresenceTag dgg=offline.dgg/>
        }
        .into_any(),
    };
    let sub = move || match streamer.get() {
        StreamerView::Live(live) => {
            let started = live.started_at as f64;
            let viewers = match live.viewer_count {
                Some(count) => Some(view! { <span>{format!("{} watching", format_compact_number(count as f64))}</span> }.into_any()),
                None => (live.max_viewer_count > 0).then(|| view! {
                    <span>{format!("{} peak viewers this stream", format_compact_number(live.max_viewer_count as f64))}</span>
                }.into_any()),
            };
            let sources = (live.sources.len() > 1).then(|| {
                live.sources
                    .iter()
                    .filter_map(|source| {
                        let count = source.viewer_count?;
                        Some(view! {
                            <span>
                                <PlatformIcon platform=source.platform.clone() size=12/>
                                {format!(" {} {}", format_compact_number(count as f64), source.platform)}
                            </span>
                        })
                    })
                    .collect_view()
            });
            view! {
                <div class="streamer-sub">
                    <span class="streamer-title-text">{live.title.clone()}</span>
                    <span class="muted meta-row">
                        <span>{move || format_uptime(now.get() - started)}</span>
                        {viewers}
                        {sources}
                        {live.category.clone().map(|c| view! { <span>{c}</span> })}
                    </span>
                </div>
            }
            .into_any()
        }
        StreamerView::Offline(offline) => {
            let text = match offline.last_ended_at {
                Some(ended) => {
                    let duration = offline
                        .last_started_at
                        .map(|started| {
                            format!(" for {}", format_duration((ended - started) as f64))
                        })
                        .unwrap_or_default();
                    let peak = match offline.last_max_viewer_count {
                        Some(peak) if peak != 0 => {
                            format!(", peak {} viewers", format_compact_number(peak as f64))
                        }
                        _ => String::new(),
                    };
                    format!(
                        "Last live {}{duration}{peak}",
                        format_relative(ended as f64)
                    )
                }
                None => "No streams seen yet".to_owned(),
            };
            view! { <div class="streamer-sub muted">{text}</div> }.into_any()
        }
    };
    let bindings = move || {
        let s = streamer.get();
        let (id, bindings) = match &s {
            StreamerView::Live(l) => (l.id.clone(), l.bindings.clone()),
            StreamerView::Offline(o) => (o.id.clone(), o.bindings.clone()),
        };
        view! {
            <Link
                class="binding-chip intelligence-details-link"
                to=format!("/streamers/{}/intelligence", encode_uri_component(&id))
            >
                "Intelligence Details"
            </Link>
            {bindings
                .into_iter()
                .map(|binding| view! {
                    <a class="binding-chip" href=binding.url.clone() target="_blank" rel="noreferrer">
                        <PlatformIcon platform=binding.platform.clone() size=14/>
                        <span>{binding.username.clone()}</span>
                    </a>
                })
                .collect_view()}
        }
    };
    view! {
        <div class="page-header streamer-header">
            <div>
                <h1>{title}</h1>
                {sub}
            </div>
            <div class="streamer-bindings">{bindings}</div>
        </div>
    }
}

#[component]
fn LivestreamIntelligencePanel(
    streamer_id: String,
    #[prop(into)] intelligence: Signal<Option<LivestreamIntelligence>>,
) -> impl IntoView {
    let submitted = RwSignal::new(false);
    let alert_id = Memo::new(move |_| {
        intelligence.with(|i| {
            i.as_ref()
                .and_then(|i| i.latest_alert.as_ref().map(|a| a.alert_id.clone()))
        })
    });
    Effect::new(move |_| {
        alert_id.track();
        submitted.set(false);
    });
    let id = streamer_id.clone();
    let submit = move |verdict: FeedbackVerdict| {
        let Some(alert) = alert_id.get_untracked() else {
            return;
        };
        submitted.set(true);
        let id = id.clone();
        spawn_detached(async move {
            if api::submit_livestream_feedback(&id, &alert, verdict, None)
                .await
                .is_err()
            {
                submitted.set(false);
            }
        });
    };
    let details_to = format!(
        "/streamers/{}/intelligence",
        encode_uri_component(&streamer_id)
    );
    move || {
        let details_to = details_to.clone();
        let submit = submit.clone();
        intelligence.get().map(|intelligence| {
            let chapters: Vec<_> = intelligence.chapters.iter().rev().take(8).cloned().collect();
            let summary_text = intelligence
                .summary
                .as_ref()
                .map(|s| s.text.clone())
                .or_else(|| intelligence.semantic.as_ref().map(|s| s.headline.clone()))
                .unwrap_or_else(|| "Building a current summary…".to_owned());
            let alert = intelligence.latest_alert.clone();
            let feedback = alert.map(|alert| {
                let (s1, s2, s3) = (submit.clone(), submit.clone(), submit.clone());
                view! {
                    <div class="intelligence-alert-feedback">
                        <div>
                            <strong>{format!("Latest Alert: {}", alert.title)}</strong>
                            <span>{alert.reason.clone()}</span>
                        </div>
                        {move || {
                            let (s1, s2, s3) = (s1.clone(), s2.clone(), s3.clone());
                            if submitted.get() {
                                view! { <span class="muted">"Feedback saved"</span> }.into_any()
                            } else {
                                view! {
                                    <div class="intelligence-feedback-actions">
                                        <button type="button" on:click=move |_| s1(FeedbackVerdict::Useful)>"Useful"</button>
                                        <button type="button" on:click=move |_| s2(FeedbackVerdict::NotUseful)>"Not Useful"</button>
                                        <button type="button" on:click=move |_| s3(FeedbackVerdict::FalsePositive)>"False Positive"</button>
                                    </div>
                                }
                                .into_any()
                            }
                        }}
                    </div>
                }
            });
            view! {
                <section class="page-section intelligence-panel">
                    <h2 class="section-title intelligence-panel-title">
                        "Live Intelligence"
                        <Link class="section-action-link" to=details_to>"Details ›"</Link>
                    </h2>
                    <div class="intelligence-summary-card">
                        <div class="intelligence-summary-heading">
                            <strong>
                                {intelligence.summary.as_ref().map_or_else(|| "Current Read".to_owned(), |s| s.topic.clone())}
                            </strong>
                            <span class="intelligence-score">
                                {format!("Relevance {}", number_string(intelligence.relevance_score))}
                            </span>
                        </div>
                        <p>{summary_text}</p>
                        <div class="meta-row">
                            {intelligence.summary.as_ref().map(|s| view! {
                                <span>{format!("Updated {}", format_relative(s.updated_at as f64))}</span>
                            })}
                            {intelligence.trend.as_ref().filter(|t| t.anomalous).map(|t| view! {
                                <span>{t.reason.clone()}</span>
                            })}
                            {intelligence
                                .destiny_presence
                                .as_ref()
                                .filter(|p| p.state == PresenceState::Confirmed)
                                .map(|p| view! {
                                    <span>
                                        {format!(
                                            "Destiny detected, {}% confidence",
                                            number_string(js_round(p.confidence * 100.0)),
                                        )}
                                    </span>
                                })}
                        </div>
                        {(!intelligence.relevance_reasons.is_empty()).then(|| view! {
                            <div class="intelligence-reasons">
                                {intelligence
                                    .relevance_reasons
                                    .iter()
                                    .map(|reason| view! { <span>{reason.clone()}</span> })
                                    .collect_view()}
                            </div>
                        })}
                    </div>
                    {(!chapters.is_empty()).then(|| view! {
                        <div class="intelligence-chapters">
                            <h3>"Recent Topics"</h3>
                            {chapters
                                .into_iter()
                                .map(|chapter| view! {
                                    <div class="intelligence-chapter">
                                        <div>
                                            <strong>{chapter.title}</strong>
                                            <span>{format_relative(chapter.started_at)}</span>
                                        </div>
                                        <p>{chapter.summary}</p>
                                    </div>
                                })
                                .collect_view()}
                        </div>
                    })}
                    {feedback}
                </section>
            }
        })
    }
}

fn find_streamer(streamers: &[StreamerView], id: &str) -> Option<StreamerView> {
    streamers.iter().find(|s| s.id() == id).cloned()
}

fn display_name(streamer: &StreamerView) -> String {
    match streamer {
        StreamerView::Live(l) => l.display_name.clone(),
        StreamerView::Offline(o) => o.display_name.clone(),
    }
}

#[component]
pub fn StreamerPage(#[prop(into)] streamer_id: String) -> impl IntoView {
    let live = use_live_data();
    let metrics = RwSignal::new(None::<StreamerMetricsResponse>);
    let error = RwSignal::new(None::<String>);
    let sessions = RwSignal::new(None::<Vec<StreamSessionView>>);
    let id = streamer_id.clone();
    let streamer = Memo::new(move |_| {
        live.snapshot
            .with(|s| s.as_ref().and_then(|s| find_streamer(&s.streamers, &id)))
    });
    Effect::new(move |_| {
        if let Some(s) = streamer.get() {
            document().set_title(&format!("{} · Omni Notify", display_name(&s)));
        }
    });

    let metrics_id = streamer_id.clone();
    spawn_scoped(async move {
        match api::fetch_streamer_metrics(&metrics_id).await {
            Ok(m) => metrics.set(Some(m)),
            Err(e) => error.set(Some(e.message().to_owned())),
        }
    });
    let sessions_id = streamer_id.clone();
    spawn_scoped(async move {
        // Session history is supplementary; the page stays useful without it.
        sessions.set(
            api::fetch_streamer_sessions(&sessions_id)
                .await
                .ok()
                .map(|r| r.sessions),
        );
    });

    let has_snapshot = Memo::new(move |_| live.snapshot.with(Option::is_some));
    let present = Memo::new(move |_| streamer.with(Option::is_some));
    let intelligence = Memo::new(move |_| {
        streamer.with(|s| match s {
            Some(StreamerView::Live(live)) => streamer_intelligence(live),
            _ => None,
        })
    });
    let page_id = streamer_id.clone();
    move || {
        if !has_snapshot.get() {
            return view! { <div class="loading">"Loading…"</div> }.into_any();
        }
        if !present.get() {
            let id = page_id.clone();
            return view! {
                <Link to="/" class="back-link">"← Home"</Link>
                <div class="error">
                    <div>"Unknown streamer"</div>
                    <div class="error-detail">{format!("No channel named “{id}” is being monitored.")}</div>
                </div>
            }
            .into_any();
        }
        let current = Signal::derive(move || {
            streamer
                .get()
                .unwrap_or_else(|| StreamerView::Offline(empty_offline()))
        });
        view! {
            <Link to="/" class="back-link">"← Home"</Link>
            <StreamerHeader streamer=current/>
            <LivestreamIntelligencePanel streamer_id=page_id.clone() intelligence/>
            {move || error.get().map(|e| view! { <div class="error-inline">{format!("Failed to load viewer metrics: {e}")}</div> })}
            {move || {
                match (metrics.get(), error.with(Option::is_some)) {
                    (None, false) => Some(view! { <div class="loading-inline">"Loading viewer metrics…"</div> }.into_any()),
                    (None, true) => None,
                    (Some(m), _) => {
                        let has_metrics = !m.daily_buckets.is_empty() || m.all_time_max > 0;
                        Some(if has_metrics {
                            view! {
                                <StreamerStats metrics=m.clone()/>
                                <PlatformStats metrics=m.clone()/>
                                <ViewerChart metrics=m/>
                            }
                            .into_any()
                        } else {
                            view! { <div class="no-data">"No viewer data recorded yet"</div> }.into_any()
                        })
                    }
                }
            }}
            {move || {
                sessions
                    .get()
                    .filter(|s| !s.is_empty())
                    .map(|sessions| view! { <RecentSessions sessions/> })
            }}
        }
        .into_any()
    }
}

fn empty_offline() -> omni_api::streamers::OfflineStreamerView {
    omni_api::streamers::OfflineStreamerView {
        id: String::new(),
        display_name: String::new(),
        bindings: Vec::new(),
        tier: Default::default(),
        dgg: None,
        live: omni_api::streamers::LiveFalse,
        last_started_at: None,
        last_ended_at: None,
        last_max_viewer_count: None,
    }
}
