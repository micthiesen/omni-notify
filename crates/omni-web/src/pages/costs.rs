//! Usage and estimated spend (`pages/CostsPage.tsx`).

use leptos::prelude::*;
use omni_api::costs::{
    CostCategory, CostPriceStatus, CostRange, CostRecentEvent, CostServiceRow, CostsResponse,
};
use omni_web_kit::api;
use omni_web_kit::charts::{AxisStyle, BarChart, BarPoint, BarSeries};
use omni_web_kit::task::spawn_scoped;
use omni_web_kit::utils::format::{
    format_absolute_with_year, format_calendar_date, format_cents, format_compact_number,
    to_title_case,
};
use omni_web_kit::utils::js::locale_number;

const RANGES: [CostRange; 4] = [
    CostRange::Days(7),
    CostRange::Days(30),
    CostRange::Days(90),
    CostRange::All,
];
const FEATURE_COLORS: [&str; 8] = [
    "#38bdf8", "#a770ff", "#4ade80", "#fbbf24", "#fb7185", "#2dd4bf", "#f97316", "#818cf8",
];

pub fn cost_label(value: &str) -> String {
    match value.to_lowercase().as_str() {
        "api" => "API".to_owned(),
        "llm" => "LLM".to_owned(),
        "openai" => "OpenAI".to_owned(),
        "self-hosted" => "Self-Hosted".to_owned(),
        "stt" => "STT".to_owned(),
        "tts" => "TTS".to_owned(),
        _ => to_title_case(value),
    }
}

fn category_str(category: CostCategory) -> &'static str {
    match category {
        CostCategory::Llm => "llm",
        CostCategory::Search => "search",
        CostCategory::Tts => "tts",
        CostCategory::Retrieval => "retrieval",
        CostCategory::Transcription => "transcription",
    }
}

fn price_status_str(status: CostPriceStatus) -> &'static str {
    match status {
        CostPriceStatus::Priced => "priced",
        CostPriceStatus::Estimated => "estimated",
        CostPriceStatus::Free => "free",
        CostPriceStatus::Unknown => "unknown",
    }
}

fn event_count_label(count: u64) -> String {
    format!(
        "{} {}",
        locale_number(count as f64),
        if count == 1 { "event" } else { "events" }
    )
}

fn range_label(range: CostRange) -> String {
    match range {
        CostRange::All => "All".to_owned(),
        CostRange::Days(days) => format!("{days}d"),
    }
}

fn range_text(range: CostRange) -> String {
    match range {
        CostRange::All => "all-time".to_owned(),
        CostRange::Days(days) => format!("{days}-day"),
    }
}

fn usage_parts(service: &CostServiceRow) -> Vec<String> {
    let usage = &service.usage;
    let mut parts = Vec::new();
    if usage.input_tokens != 0.0 || usage.output_tokens != 0.0 {
        parts.push(format!(
            "{} in / {} out",
            format_compact_number(usage.input_tokens),
            format_compact_number(usage.output_tokens)
        ));
    }
    if usage.characters != 0.0 {
        parts.push(format!(
            "{} characters",
            format_compact_number(usage.characters)
        ));
    }
    if usage.requests != 0.0 {
        parts.push(format!("{} requests", locale_number(usage.requests)));
    }
    if usage.credits != 0.0 {
        parts.push(format!("{} credits", locale_number(usage.credits)));
    }
    parts
}

fn event_cost(event: &CostRecentEvent) -> String {
    if event.price_status == CostPriceStatus::Free {
        return "Free".to_owned();
    }
    match event.cost_cents {
        Some(cents) => format_cents(Some(cents)).unwrap_or_else(|| "Unknown".to_owned()),
        None => cost_label(price_status_str(event.price_status)),
    }
}

#[component]
fn DailyChart(data: CostsResponse) -> impl IntoView {
    let features: Vec<String> = data.by_feature.iter().map(|f| f.feature.clone()).collect();
    let has_priced = data.daily.iter().any(|d| d.priced_event_count > 0);
    if !has_priced || data.daily.is_empty() || features.is_empty() {
        return view! { <div class="no-data">"No priced cost data in this range"</div> }.into_any();
    }
    let series: Vec<BarSeries> = features
        .iter()
        .enumerate()
        .map(|(i, f)| BarSeries {
            key: f.clone(),
            color: FEATURE_COLORS[i % FEATURE_COLORS.len()].to_owned(),
        })
        .collect();
    let points: Vec<BarPoint> = data
        .daily
        .iter()
        .map(|day| BarPoint {
            x: day.date.clone(),
            values: features
                .iter()
                .map(|f| day.by_feature.get(f).copied().unwrap_or(0.0))
                .collect(),
        })
        .collect();
    let legend = series
        .iter()
        .map(|s| {
            view! {
                <span>
                    <i style=format!("background: {}", s.color)></i>
                    {to_title_case(&s.key)}
                </span>
            }
        })
        .collect_view();
    let tooltip_points = points.clone();
    let tooltip_series = series.clone();
    let tooltip = Callback::new(move |index: usize| {
        let Some(point) = tooltip_points.get(index) else {
            return ().into_any();
        };
        let rows = tooltip_series
            .iter()
            .zip(&point.values)
            .map(|(s, value)| {
                view! {
                    <div class="tooltip-row">
                        <i class="costs-tooltip-swatch" style=format!("background: {}", s.color)></i>
                        <span>
                            {format!(
                                "{}: {}",
                                to_title_case(&s.key),
                                format_cents(Some(*value)).unwrap_or_default(),
                            )}
                        </span>
                    </div>
                }
            })
            .collect_view();
        view! {
            <div class="custom-tooltip">
                <div class="tooltip-label">{format_calendar_date(&point.x, true)}</div>
                {rows}
            </div>
        }
        .into_any()
    });
    view! {
        <div class="costs-chart-legend">{legend}</div>
        <div class="chart-container costs-chart">
            <BarChart
                data=Signal::stored(points)
                series=Signal::stored(series)
                x_tick=Callback::new(|date: String| format_calendar_date(&date, false))
                y_tick=Callback::new(|value: f64| format_cents(Some(value)).unwrap_or_default())
                tooltip
                y_width=62.0
                max_bar_size=36.0
                axis=AxisStyle::default()
            />
        </div>
    }
    .into_any()
}

#[component]
fn CostsContent(data: CostsResponse) -> impl IntoView {
    let summary = data.summary.clone();
    let highest = summary.highest_day.clone();
    let usage = {
        let tokens = (summary.input_tokens > 0.0 || summary.output_tokens > 0.0).then(|| {
            view! {
                <span>{format!("{} input tokens", format_compact_number(summary.input_tokens))}</span>
                <span>{format!("{} output tokens", format_compact_number(summary.output_tokens))}</span>
            }
        });
        let characters = (summary.characters > 0.0).then(|| {
            view! { <span>{format!("{} characters", format_compact_number(summary.characters))}</span> }
        });
        let credits = (summary.credits > 0.0).then(
            || view! { <span>{format!("{} credits", locale_number(summary.credits))}</span> },
        );
        view! {
            <span>{format!("{} requests", locale_number(summary.requests))}</span>
            {tokens}
            {characters}
            {credits}
        }
    };
    let features = data
        .by_feature
        .iter()
        .map(|item| {
            let unpriced = if item.unknown_event_count > 0 {
                format!(
                    ", {} unpriced",
                    locale_number(item.unknown_event_count as f64)
                )
            } else {
                String::new()
            };
            view! {
                <div class="costs-row">
                    <div class="costs-row-main">
                        <strong>{to_title_case(&item.feature)}</strong>
                        <span>{format!("{}{unpriced}", event_count_label(item.event_count))}</span>
                    </div>
                    <span class="costs-row-value">{format_cents(Some(item.cost_cents))}</span>
                </div>
            }
        })
        .collect_view();
    let services = data
        .by_service
        .iter()
        .map(|item| {
            let usage = usage_parts(item);
            let value = if item.unknown_event_count == item.event_count {
                "Unpriced".to_owned()
            } else if item.cost_cents == 0.0 {
                "Free".to_owned()
            } else {
                format_cents(Some(item.cost_cents)).unwrap_or_default()
            };
            view! {
                <div class="costs-row">
                    <div class="costs-row-main">
                        <strong>{cost_label(&item.service)}</strong>
                        <span class="costs-model">
                            {item.model.clone().unwrap_or_else(|| cost_label(category_str(item.category)))}
                        </span>
                        {(!usage.is_empty()).then(|| view! {
                            <span class="meta-row costs-service-usage">
                                {usage.into_iter().map(|part| view! { <span>{part}</span> }).collect_view()}
                            </span>
                        })}
                        {(item.unknown_event_count > 0).then(|| view! {
                            <span>{format!("{} unpriced", locale_number(item.unknown_event_count as f64))}</span>
                        })}
                    </div>
                    <span class="costs-row-value">{value}</span>
                </div>
            }
        })
        .collect_view();
    let recent = data
        .recent
        .iter()
        .map(|event| {
            view! {
                <div class="costs-event-row">
                    <div class="costs-event-when">
                        <strong>{format_absolute_with_year(event.incurred_at as f64)}</strong>
                        <span>{cost_label(&event.feature)}</span>
                    </div>
                    <div class="costs-event-detail">
                        <strong>{cost_label(&event.operation)}</strong>
                        <span class="meta-row">
                            <span>{cost_label(&event.service)}</span>
                            {event.model.clone().map(|m| view! { <span>{m}</span> })}
                            <span>{cost_label(category_str(event.category))}</span>
                        </span>
                    </div>
                    <span class=format!(
                        "costs-event-value {}",
                        if event.cost_cents.is_none() { "unknown" } else { "" },
                    )>{event_cost(event)}</span>
                </div>
            }
        })
        .collect_view();
    let no_features = data.by_feature.is_empty();
    let no_services = data.by_service.is_empty();
    let no_recent = data.recent.is_empty();
    view! {
        <div class="stat-strip costs-stat-strip">
            <div class="stat-tile accent">
                <span class="stat-label">"Selected Total"</span>
                <span class="stat-value">{format_cents(Some(summary.selected_cost_cents))}</span>
                <span class="stat-detail">
                    {format!("{} tracked events", locale_number(summary.event_count as f64))}
                </span>
            </div>
            <div class="stat-tile">
                <span class="stat-label">"Daily Average"</span>
                <span class="stat-value">{format_cents(Some(summary.average_daily_cost_cents))}</span>
                <span class="stat-detail">"in selected range"</span>
            </div>
            <div class="stat-tile">
                <span class="stat-label">"Highest Day"</span>
                <span class="stat-value">
                    {format_cents(highest.as_ref().map(|d| d.cost_cents)).unwrap_or_else(|| "—".to_owned())}
                </span>
                <span class="stat-detail">
                    {highest
                        .as_ref()
                        .map(|d| format_calendar_date(&d.date, true))
                        .unwrap_or_else(|| "No priced usage".to_owned())}
                </span>
            </div>
            <div class="stat-tile">
                <span class="stat-label">"All-Time Total"</span>
                <span class="stat-value">{format_cents(Some(summary.all_time_cost_cents))}</span>
                <span class="stat-detail">
                    {if summary.all_time_unknown_event_count > 0 {
                        format!(
                            "{} unpriced excluded",
                            locale_number(summary.all_time_unknown_event_count as f64),
                        )
                    } else {
                        "all recorded usage".to_owned()
                    }}
                </span>
            </div>
            <div class=format!(
                "stat-tile {}",
                if summary.unknown_event_count != 0 { "danger" } else { "" },
            )>
                <span class="stat-label">"Unpriced Events"</span>
                <span class="stat-value">{locale_number(summary.unknown_event_count as f64)}</span>
                <span class="stat-detail">"excluded from totals"</span>
            </div>
        </div>

        <div class="meta-row costs-usage-summary">{usage}</div>

        <section class="page-section">
            <h2 class="section-title">"Daily Spend by Feature"</h2>
            <DailyChart data=data.clone()/>
        </section>

        <div class="costs-breakdown-grid">
            <section class="page-section">
                <h2 class="section-title">"By Feature"</h2>
                <div class="costs-list">
                    {features}
                    {no_features.then(|| view! { <div class="muted">"No feature costs in this range."</div> })}
                </div>
            </section>
            <section class="page-section">
                <h2 class="section-title">"By Service"</h2>
                <div class="costs-list">
                    {services}
                    {no_services.then(|| view! { <div class="muted">"No service costs in this range."</div> })}
                </div>
            </section>
        </div>

        <section class="page-section">
            <h2 class="section-title">"Recent Cost Events"</h2>
            <div class="costs-event-list">
                {recent}
                {no_recent.then(|| view! { <div class="muted">"No recent cost events in this range."</div> })}
            </div>
        </section>
    }
}

#[component]
pub fn CostsPage() -> impl IntoView {
    let range = RwSignal::new(CostRange::Days(30));
    let data = RwSignal::new(None::<CostsResponse>);
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(true);
    let displayed_range = RwSignal::new(CostRange::Days(30));

    Effect::new(move |_| {
        let selected = range.get();
        loading.set(true);
        error.set(None);
        spawn_scoped(async move {
            match api::fetch_costs(selected).await {
                Ok(next) => {
                    data.set(Some(next));
                    displayed_range.set(selected);
                }
                Err(e) => error.set(Some(e.message().to_owned())),
            }
            loading.set(false);
        });
    });

    let range_buttons = RANGES
        .into_iter()
        .map(|item| {
            view! {
                <button
                    type="button"
                    class=move || format!("range-btn {}", if range.get() == item { "active" } else { "" })
                    aria-pressed=move || (range.get() == item).to_string()
                    on:click=move |_| range.set(item)
                >
                    {range_label(item)}
                </button>
            }
        })
        .collect_view();

    view! {
        <div class="page-header costs-header">
            <div class="page-header-stack">
                <h1>"Costs"</h1>
                <p class="page-subtitle">"Usage and estimated spend across Omni Notify services."</p>
            </div>
            <div class="range-buttons" aria-label="Cost date range">{range_buttons}</div>
        </div>
        {move || {
            let has_data = data.with(Option::is_some);
            let err = error.get();
            match (has_data, err) {
                (false, None) => Some(view! { <div class="loading">"Loading…"</div> }.into_any()),
                (false, Some(err)) => Some(
                    view! {
                        <div class="error">
                            <div>"Failed to load costs"</div>
                            <div class="error-detail">{err}</div>
                        </div>
                    }
                    .into_any(),
                ),
                (true, Some(err)) => Some(
                    view! {
                        <div role="alert" class="error-inline">
                            {format!("{err}. Showing {} data.", range_text(displayed_range.get()))}
                        </div>
                    }
                    .into_any(),
                ),
                (true, None) => None,
            }
        }}
        {move || {
            (loading.get() && data.with(Option::is_some))
                .then(|| view! {
                    <div class="stale-note muted" role="status">
                        {format!("Updating… Showing {} data.", range_text(displayed_range.get()))}
                    </div>
                })
        }}
        {move || data.get().map(|data| view! { <CostsContent data/> })}
    }
}
