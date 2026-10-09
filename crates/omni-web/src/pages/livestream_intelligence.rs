//! What the intelligence pipeline is doing, why, and what it cost.
//! Refreshes every 10 s.

use std::time::Duration;

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::intelligence::{
    EventKind, EventStatus, IntelligenceDetailsResponse, LivestreamEvent, MetricMap,
    PipelineStatus, StageDiagnostic,
};
use omni_api::streamers::StreamerView;
use omni_web_kit::api;
use omni_web_kit::hooks::use_now;
use omni_web_kit::router::Link;
use omni_web_kit::task::{sleep, spawn_scoped};
use omni_web_kit::use_live_data;
use omni_web_kit::utils::format::{format_duration, format_relative};
use omni_web_kit::utils::js::{js_round, number_string, to_fixed};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelineFilter {
    Key,
    All,
    Voice,
    Alerts,
    Errors,
}

const STAGES: [(&str, &str); 3] = [
    ("voice", "Voice Detection"),
    ("summary", "Now Summary"),
    ("alert", "Alerts"),
];
const FILTERS: [(TimelineFilter, &str); 5] = [
    (TimelineFilter::Key, "Key Events"),
    (TimelineFilter::All, "All"),
    (TimelineFilter::Voice, "Voice"),
    (TimelineFilter::Alerts, "Alerts"),
    (TimelineFilter::Errors, "Errors"),
];

fn money_from_cents(cents: f64) -> String {
    format!(
        "${}",
        to_fixed(cents / 100.0, if cents < 1.0 { 4 } else { 2 })
    )
}

fn status_str(status: PipelineStatus) -> &'static str {
    match status {
        PipelineStatus::Idle => "idle",
        PipelineStatus::Running => "running",
        PipelineStatus::Success => "success",
        PipelineStatus::Skipped => "skipped",
        PipelineStatus::Error => "error",
    }
}

fn event_status_str(status: EventStatus) -> &'static str {
    match status {
        EventStatus::Info => "info",
        EventStatus::Success => "success",
        EventStatus::Warning => "warning",
        EventStatus::Error => "error",
    }
}

fn event_kind_str(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Session => "session",
        EventKind::Metadata => "metadata",
        EventKind::Voice => "voice",
        EventKind::Summary => "summary",
        EventKind::Alert => "alert",
        EventKind::Feedback => "feedback",
        EventKind::Anomaly => "anomaly",
    }
}

fn stage_status(stage: Option<&StageDiagnostic>) -> String {
    match stage {
        None => "Not Run".to_owned(),
        Some(stage) if stage.eligible == Some(false) => "Not Eligible".to_owned(),
        Some(stage) => status_str(stage.status).replace('_', " "),
    }
}

/// "voicePrintScore" → "Voice Print Score".
pub fn metric_label(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let mut out = String::new();
    for (i, ch) in chars.iter().enumerate() {
        if i > 0 && chars[i - 1].is_ascii_lowercase() && ch.is_ascii_uppercase() {
            out.push(' ');
        }
        out.push(*ch);
    }
    let mut it = out.chars();
    match it.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), it.as_str()),
        None => out,
    }
}

fn metric_value(value: &Value) -> String {
    match value {
        Value::Number(n) => number_string(js_round(n.as_f64().unwrap_or(0.0) * 1000.0) / 1000.0),
        Value::String(s) => s.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn metrics_list(metrics: &MetricMap) -> impl IntoView + use<> {
    let rows = metrics
        .iter()
        .map(|(key, value)| {
            view! {
                <div>
                    <dt>{metric_label(key)}</dt>
                    <dd>{metric_value(value)}</dd>
                </div>
            }
        })
        .collect_view();
    view! { <dl class="intelligence-metrics">{rows}</dl> }
}

#[component]
fn PipelineStageCard(
    label: &'static str,
    stage: Option<StageDiagnostic>,
    now: f64,
) -> impl IntoView {
    let class = format!(
        "intelligence-stage-card stage-{}",
        stage.as_ref().map_or("idle", |s| status_str(s.status))
    );
    let status = stage_status(stage.as_ref());
    let detail = stage
        .as_ref()
        .and_then(|s| s.detail.clone())
        .unwrap_or_else(|| "No diagnostic state has been recorded yet.".to_owned());
    let meta = stage.as_ref().map(|s| {
        view! {
            {s.finished_at.filter(|f| *f != 0).map(|f| view! { <span>{format!("Last {}", format_relative(f as f64))}</span> })}
            {(s.status == PipelineStatus::Running)
                .then_some(s.started_at)
                .flatten()
                .filter(|v| *v != 0)
                .map(|started| view! { <span>{format!("Running {}", format_duration(now - started as f64))}</span> })}
            {s.next_at.filter(|n| *n != 0).map(|next| view! {
                <span>
                    {format!(
                        "{} {}",
                        if (next as f64) <= now { "Due" } else { "Next" },
                        format_relative(next as f64),
                    )}
                </span>
            })}
            {s.duration_ms.map(|d| view! { <span>{format!("{} processing", format_duration(d as f64))}</span> })}
        }
    });
    let metrics = stage
        .as_ref()
        .and_then(|s| s.metrics.as_ref())
        .filter(|m| !m.is_empty())
        .map(metrics_list);
    view! {
        <article class=class>
            <div class="intelligence-stage-heading">
                <h3>{label}</h3>
                <span class="intelligence-stage-status">{status}</span>
            </div>
            <p>{detail}</p>
            <div class="meta-row intelligence-stage-meta">{meta}</div>
            {metrics}
        </article>
    }
}

fn event_visible(event: &LivestreamEvent, filter: TimelineFilter) -> bool {
    match filter {
        TimelineFilter::All => true,
        TimelineFilter::Voice => event.kind == EventKind::Voice,
        TimelineFilter::Alerts => matches!(event.kind, EventKind::Alert | EventKind::Feedback),
        TimelineFilter::Errors => event.status == EventStatus::Error,
        TimelineFilter::Key => {
            event.kind != EventKind::Session || event.status != EventStatus::Info
        }
    }
}

#[component]
fn TimelineEvent(event: LivestreamEvent) -> impl IntoView {
    let status = event_status_str(event.status);
    view! {
        <li class=format!("intelligence-event event-{status}")>
            <span class="intelligence-event-dot" aria-hidden="true"></span>
            <div class="intelligence-event-body">
                <div class="intelligence-event-heading">
                    <strong>{event.title.clone()}</strong>
                    <span>{format_relative(event.created_at as f64)}</span>
                </div>
                {event.detail.clone().filter(|d| !d.is_empty()).map(|d| view! { <p>{d}</p> })}
                <div class="meta-row">
                    <span>{event_kind_str(event.kind).replace('_', " ")}</span>
                    <span>{status}</span>
                    {event.duration_ms.map(|d| view! { <span>{format!("{} processing", format_duration(d as f64))}</span> })}
                    {event.cost_cents.map(|c| view! {
                        <span>{if c == 0.0 { "$0 local".to_owned() } else { money_from_cents(c) }}</span>
                    })}
                </div>
                {event.metrics.as_ref().filter(|m| !m.is_empty()).map(|m| view! {
                    <details class="intelligence-event-evidence">
                        <summary>"Evidence"</summary>
                        {metrics_list(m)}
                    </details>
                })}
            </div>
        </li>
    }
}

fn stage_of(details: &IntelligenceDetailsResponse, key: &str) -> Option<StageDiagnostic> {
    let value = details.diagnostics.as_ref()?.stages.get(key)?;
    serde_json::from_value(value.clone()).ok()
}

#[component]
pub fn LivestreamIntelligencePage(#[prop(into)] streamer_id: String) -> impl IntoView {
    let live = use_live_data();
    let now = use_now(1_000);
    let details = RwSignal::new(None::<IntelligenceDetailsResponse>);
    let error = RwSignal::new(None::<String>);
    let filter = RwSignal::new(TimelineFilter::Key);
    let id = streamer_id.clone();
    let streamer = Memo::new(move |_| {
        live.snapshot.with(|s| {
            s.as_ref()
                .and_then(|s| s.streamers.iter().find(|x| x.id() == id).cloned())
        })
    });

    let fetch_id = streamer_id.clone();
    spawn_scoped(async move {
        loop {
            match api::fetch_livestream_intelligence_details(&fetch_id, 100).await {
                Ok(next) => {
                    details.set(Some(next));
                    error.set(None);
                }
                Err(e) => error.set(Some(e.message().to_owned())),
            }
            sleep(Duration::from_secs(10)).await;
        }
    });
    Effect::new(move |_| {
        if let Some(s) = streamer.get() {
            let name = match &s {
                StreamerView::Live(l) => l.display_name.clone(),
                StreamerView::Offline(o) => o.display_name.clone(),
            };
            document().set_title(&format!("{name} Intelligence · Omni Notify"));
        }
    });

    let back_to = format!("/streamers/{}", encode_uri_component(&streamer_id));
    move || {
        let has_snapshot = live.snapshot.with(Option::is_some);
        let has_details = details.with(Option::is_some);
        let err = error.get();
        if !has_snapshot || (!has_details && err.is_none()) {
            return view! { <div class="loading">"Loading…"</div> }.into_any();
        }
        let Some(current) = streamer.get() else {
            return view! { <div class="error-banner">"Streamer not found."</div> }.into_any();
        };
        let Some(data) = details.get() else {
            return view! { <div class="error-banner">{err}</div> }.into_any();
        };
        let (name, is_live) = match &current {
            StreamerView::Live(l) => (l.display_name.clone(), true),
            StreamerView::Offline(o) => (o.display_name.clone(), false),
        };
        let runtime = data.runtime.clone();
        let queue_total = runtime.as_ref().map_or(0, |r| {
            [r.queues.capture, r.queues.speech, r.queues.llm]
                .iter()
                .map(|q| q.running + q.queued)
                .sum::<u64>()
        });
        let has_stage_error = data.diagnostics.as_ref().is_some_and(|d| {
            d.stages
                .values()
                .any(|stage| stage.get("status").and_then(Value::as_str) == Some("error"))
        });
        let budget_percent = runtime.as_ref().map_or(0.0, |r| {
            if r.budget.limit_cents > 0.0 {
                (r.budget.spent_cents / r.budget.limit_cents * 100.0).min(100.0)
            } else {
                100.0
            }
        });
        let intelligence = data.intelligence.clone();
        let evidence = intelligence.as_ref().map(|intel| {
            let transcript = match &intel.summary {
                Some(summary) => view! {
                    <strong>{summary.topic.clone()}</strong>
                    <p class="intelligence-transcript-excerpt">{summary.transcript_excerpt.clone()}</p>
                    <div class="meta-row">
                        <span>{format!("{}% confidence", number_string(js_round(summary.confidence * 100.0)))}</span>
                        <span>{format!("{}s window", number_string(summary.window_seconds))}</span>
                    </div>
                }
                .into_any(),
                None => view! { <p>"No transcript-backed summary has been produced for this session."</p> }.into_any(),
            };
            let surge = match &intel.trend {
                Some(trend) => {
                    let headline = if trend.anomalous {
                        "Confirmed".to_owned()
                    } else {
                        trend.suppression_reason.clone().unwrap_or_else(|| "No unusual rise".to_owned())
                    };
                    let platform = format!(
                        "Platform: {}{}",
                        trend.current_viewers.map_or_else(|| "Unavailable".to_owned(), number_string),
                        trend
                            .baseline_viewers
                            .map(|b| format!(" vs {} baseline", number_string(js_round(b))))
                            .unwrap_or_default()
                    );
                    let dgg = trend.current_dgg_viewers.map(|current| {
                        format!(
                            "DGG: {}{}",
                            number_string(current),
                            trend
                                .baseline_dgg_viewers
                                .map(|b| format!(" vs {} baseline", number_string(js_round(b))))
                                .unwrap_or_default()
                        )
                    });
                    let typical = trend
                        .typical_peak_viewers
                        .map(|p| format!("Typical session peak: {}", number_string(js_round(p))));
                    view! {
                        <strong>{headline}</strong>
                        <p>{platform}</p>
                        {dgg.map(|d| view! { <p>{d}</p> })}
                        {typical.map(|t| view! { <p>{t}</p> })}
                        <div class="meta-row">
                            <span>{format!("{} baseline samples", number_string(trend.baseline_samples.unwrap_or(0.0)))}</span>
                            <span>
                                {format!(
                                    "{} of 2 confirmations",
                                    number_string(trend.candidate_observations.unwrap_or(0.0).min(2.0)),
                                )}
                            </span>
                        </div>
                    }
                    .into_any()
                }
                None => view! { <p>"No viewer baseline has been collected for this session."</p> }.into_any(),
            };
            view! {
                <section class="page-section intelligence-current-evidence">
                    <h2 class="section-title">"Current Evidence"</h2>
                    <div class="intelligence-evidence-grid">
                        <article>
                            <h3>"Latest Transcript Window"</h3>
                            {transcript}
                        </article>
                        <article>
                            <h3>"Viewer Surge"</h3>
                            {surge}
                        </article>
                    </div>
                </section>
            }
        });
        let stages_data = data.clone();
        let stages = move || {
            let now = now.get();
            STAGES
                .iter()
                .map(|(key, label)| {
                    let stage = stage_of(&stages_data, key);
                    view! { <PipelineStageCard label=*label stage now/> }
                })
                .collect_view()
        };
        let events_data = data.events.clone();
        let events = move || {
            let selected = filter.get();
            let visible: Vec<LivestreamEvent> = events_data
                .iter()
                .filter(|e| event_visible(e, selected))
                .cloned()
                .collect();
            if visible.is_empty() {
                view! { <div class="no-data">"No events match this filter yet."</div> }.into_any()
            } else {
                view! {
                    <ol class="intelligence-event-list">
                        {visible.into_iter().map(|event| view! { <TimelineEvent event/> }).collect_view()}
                    </ol>
                }
                .into_any()
            }
        };
        let filter_buttons = FILTERS
            .into_iter()
            .map(|(value, label)| view! {
                <button
                    type="button"
                    class=move || format!("chip-btn {}", if filter.get() == value { "active" } else { "" })
                    on:click=move |_| filter.set(value)
                >
                    {label}
                </button>
            })
            .collect_view();
        view! {
            <div class="page-header intelligence-details-header">
                <div class="page-header-stack">
                    <Link class="back-link" to=back_to.clone()>{format!("← {name}")}</Link>
                    <h1>"Intelligence Details"</h1>
                    <p class="page-subtitle">
                        "What the pipeline is doing, why it made each decision, and what it cost."
                    </p>
                </div>
                <span class=format!("intelligence-live-state {}", if is_live { "is-live" } else { "" })>
                    {if is_live { "Live Session" } else { "Last Session" }}
                </span>
            </div>
            {err.map(|e| view! { <div class="error-banner">{format!("Latest refresh failed: {e}")}</div> })}
            <section class="intelligence-health-grid" aria-label="Intelligence health">
                <article>
                    <span class="stat-label">"Pipeline"</span>
                    <strong>
                        {if runtime.is_some() {
                            if has_stage_error { "Needs Attention" } else { "Healthy" }
                        } else {
                            "Unavailable"
                        }}
                    </strong>
                    <span>
                        {if has_stage_error {
                            "A pipeline stage failed".to_owned()
                        } else if queue_total == 0 {
                            "Queues clear".to_owned()
                        } else {
                            format!("{queue_total} queued or running")
                        }}
                    </span>
                </article>
                <article>
                    <span class="stat-label">"Voice Model"</span>
                    <strong>{if runtime.as_ref().is_some_and(|r| r.voiceprint_loaded) { "Ready" } else { "Unavailable" }}</strong>
                    <span>
                        {runtime.as_ref().map_or_else(
                            || "No runtime connection".to_owned(),
                            |r| format!("{} of {} live targets", r.active_voice_target_count, r.active_stream_count),
                        )}
                    </span>
                </article>
                <article>
                    <span class="stat-label">"Monthly Budget"</span>
                    <strong>
                        {runtime.as_ref().map_or_else(
                            || "—".to_owned(),
                            |r| format!(
                                "{} of {}",
                                money_from_cents(r.budget.spent_cents),
                                money_from_cents(r.budget.limit_cents),
                            ),
                        )}
                    </strong>
                    <div
                        class="intelligence-budget-track"
                        aria-label=format!("{}% used", number_string(js_round(budget_percent)))
                    >
                        <span style=format!("width: {}%", number_string(budget_percent))></span>
                    </div>
                </article>
                <article>
                    <span class="stat-label">"Timeline"</span>
                    <strong>{format!("{} recent events", data.events.len())}</strong>
                    <span>
                        {format!(
                            "{} topic chapters retained",
                            intelligence.as_ref().map_or(0, |i| i.chapters.len()),
                        )}
                    </span>
                </article>
            </section>
            {evidence}
            <section class="page-section">
                <h2 class="section-title">"Current Pipeline"</h2>
                <div class="intelligence-stage-grid">{stages}</div>
            </section>
            <section class="page-section">
                <div class="intelligence-timeline-header">
                    <h2 class="section-title">"Decision Timeline"</h2>
                    <div class="intelligence-filter-row" aria-label="Timeline filters">{filter_buttons}</div>
                </div>
                {events}
            </section>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::metric_label;

    #[test]
    fn metric_labels_split_camel_case() {
        assert_eq!(metric_label("voicePrintScore"), "Voice Print Score");
        assert_eq!(metric_label("x"), "X");
    }
}
