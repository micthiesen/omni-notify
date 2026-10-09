//! Costs: usage and estimated spend by feature, service and event.

use leptos::prelude::*;
use omni_api::costs::{
    CostCategory, CostPriceStatus, CostRange, CostRecentEvent, CostServiceRow, CostsResponse,
};
use omni_web_kit::api;
use omni_web_kit::charts::{BarChart, BarPoint, BarSeries};
use omni_web_kit::components::{
    EmptyState, ErrorState, InlineNote, Meter, PageHead, Panel, Readout, ReadoutBand, ReadoutSize,
    SegOption, Segmented, SkeletonRows, Tag, Tone,
};
use omni_web_kit::task::spawn_scoped;
use omni_web_kit::utils::format::{
    format_absolute, format_absolute_with_year, format_calendar_date, format_cents,
    format_compact_number, to_title_case,
};
use omni_web_kit::utils::js::locale_number;

const RANGES: [CostRange; 4] = [
    CostRange::Days(7),
    CostRange::Days(30),
    CostRange::Days(90),
    CostRange::All,
];
const SERIES_COLORS: usize = 8;

fn series_color(index: usize) -> String {
    if index < SERIES_COLORS {
        format!("var(--series-{})", index + 1)
    } else {
        "var(--series-other)".to_owned()
    }
}

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
        CostRange::Days(days) => format!("{days}D"),
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
        return view! { <EmptyState compact=true message="No priced cost data in this range."/> }
            .into_any();
    }
    let hidden = RwSignal::new(Vec::<String>::new());
    let as_table = RwSignal::new(false);
    let all_series: Vec<BarSeries> = features
        .iter()
        .enumerate()
        .map(|(i, f)| BarSeries {
            key: f.clone(),
            color: series_color(i),
        })
        .collect();
    let daily = data.daily.clone();
    let shown_features = Memo::new(move |_| {
        let hidden = hidden.get();
        features
            .iter()
            .filter(|f| !hidden.contains(f))
            .cloned()
            .collect::<Vec<_>>()
    });
    let series = {
        let all = all_series.clone();
        Signal::derive(move || {
            let shown = shown_features.get();
            all.iter()
                .filter(|s| shown.contains(&s.key))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let points = {
        let daily = daily.clone();
        Signal::derive(move || {
            let shown = shown_features.get();
            daily
                .iter()
                .map(|day| BarPoint {
                    x: day.date.clone(),
                    values: shown
                        .iter()
                        .map(|f| day.by_feature.get(f).copied().unwrap_or(0.0))
                        .collect(),
                })
                .collect::<Vec<_>>()
        })
    };
    let tooltip = Callback::new(move |index: usize| {
        let Some(point) = points.with_untracked(|p| p.get(index).cloned()) else {
            return ().into_any();
        };
        let total: f64 = point.values.iter().sum();
        let rows = series
            .get_untracked()
            .into_iter()
            .zip(point.values.clone())
            .filter(|(_, v)| *v > 0.0)
            .map(|(s, value)| view! {
                <div class="chart-tooltip-row">
                    <i class="chart-tooltip-swatch" style=format!("background: {}", s.color)></i>
                    <span>{cost_label(&s.key)}</span>
                    <span class="num">{format_cents(Some(value)).unwrap_or_default()}</span>
                </div>
            })
            .collect_view();
        view! {
            <div class="chart-tooltip-label">{format_calendar_date(&point.x, true)}</div>
            <div class="chart-tooltip-value num">{format_cents(Some(total)).unwrap_or_default()}</div>
            {rows}
        }
        .into_any()
    });
    let legend = all_series
        .iter()
        .map(|s| {
            let key = s.key.clone();
            let toggle = key.clone();
            view! {
                <button
                    type="button"
                    class="chart-legend-item"
                    aria-pressed=move || (!hidden.with(|h| h.contains(&key))).to_string()
                    on:click=move |_| hidden.update(|h| {
                        if let Some(i) = h.iter().position(|x| *x == toggle) {
                            h.remove(i);
                        } else {
                            h.push(toggle.clone());
                        }
                    })
                >
                    <i class="chart-tooltip-swatch" style=format!("background: {}", s.color)></i>
                    {cost_label(&s.key)}
                </button>
            }
        })
        .collect_view();
    view! {
        <div class="cluster chart-toolbar">
            <div class="chart-legend" role="group" aria-label="Features">{legend}</div>
            <span class="spacer"></span>
            <Segmented
                options=Signal::derive(|| vec![SegOption::new(false, "Chart"), SegOption::new(true, "Table")])
                value=as_table
                on_change=Callback::new(move |v| as_table.set(v))
                aria_label="Show as"
                small=true
            />
        </div>
        {move || if as_table.get() {
            let shown = shown_features.get();
            view! {
                <div class="table-wrap">
                    <table class="table dense">
                        <thead><tr>
                            <th>"Day"</th>
                            {shown.iter().map(|f| view! { <th class="numeric">{cost_label(f)}</th> }).collect_view()}
                            <th class="numeric">"Total"</th>
                        </tr></thead>
                        <tbody>
                            {daily.iter().rev().map(|day| {
                                let values: Vec<f64> = shown.iter().map(|f| day.by_feature.get(f).copied().unwrap_or(0.0)).collect();
                                let total: f64 = values.iter().sum();
                                view! {
                                    <tr>
                                        <td class="num">{format_calendar_date(&day.date, true)}</td>
                                        {values.into_iter().map(|v| view! { <td class="numeric num">{if v > 0.0 { format_cents(Some(v)).unwrap_or_default() } else { "—".to_owned() }}</td> }).collect_view()}
                                        <td class="numeric num">{format_cents(Some(total)).unwrap_or_default()}</td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                </div>
            }.into_any()
        } else {
            view! {
                <div class="chart-container">
                    <BarChart
                        data=points
                        series
                        x_tick=Callback::new(|date: String| format_calendar_date(&date, false))
                        y_tick=Callback::new(|value: f64| format_cents(Some(value)).unwrap_or_default())
                        tooltip
                        y_width=62.0
                        max_bar_size=36.0
                        label="Daily spend by feature"
                    />
                </div>
            }.into_any()
        }}
    }
    .into_any()
}

#[component]
fn CostsContent(data: CostsResponse, #[prop(into)] refreshing: Signal<bool>) -> impl IntoView {
    let summary = data.summary.clone();
    let highest = summary.highest_day.clone();
    let feature_max = data
        .by_feature
        .iter()
        .map(|f| f.cost_cents)
        .fold(0.0, f64::max);
    let service_max = data
        .by_service
        .iter()
        .map(|f| f.cost_cents)
        .fold(0.0, f64::max);
    let features = data
        .by_feature
        .iter()
        .map(|item| {
            let cost = item.cost_cents;
            view! {
                <tr>
                    <td class="grow">
                        <div class="row-title">{cost_label(&item.feature)}</div>
                        <div class="row-sub">
                            {event_count_label(item.event_count)}
                            {(item.unknown_event_count > 0).then(|| format!(" · {} unpriced", locale_number(item.unknown_event_count as f64)))}
                        </div>
                    </td>
                    <td class="hide-phone"><Meter value=cost max=feature_max width=96 label=format!("{} share", cost_label(&item.feature))/></td>
                    <td class="numeric num">{format_cents(Some(cost))}</td>
                </tr>
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
                <tr>
                    <td class="grow">
                        <div class="row-title">
                            {cost_label(&item.service)} " "
                            <span class="mono dim small">{item.model.clone().unwrap_or_else(|| cost_label(category_str(item.category)))}</span>
                        </div>
                        <div class="row-sub">{usage.join(" · ")}</div>
                    </td>
                    <td class="hide-phone"><Meter value=item.cost_cents max=service_max width=96 label=format!("{} share", cost_label(&item.service))/></td>
                    <td class="numeric num">{value}</td>
                </tr>
            }
        })
        .collect_view();
    let recent = data
        .recent
        .clone()
        .into_iter()
        .map(|event| {
            view! {
                <tr>
                    <td class="num dim nowrap hide-phone">{format_absolute_with_year(event.incurred_at as f64)}</td>
                    <td class="grow">
                        <div class="row-title">{cost_label(&event.operation)}</div>
                        <div class="row-sub">
                            <span class="only-phone num">{format_absolute(event.incurred_at as f64)} " · "</span>
                            {cost_label(&event.feature)} " · " {cost_label(&event.service)}
                            {event.model.clone().map(|m| view! { " · " <span class="mono">{m}</span> })}
                        </div>
                    </td>
                    <td class="hide-phone"><Tag>{cost_label(category_str(event.category))}</Tag></td>
                    <td class=if event.cost_cents.is_none() { "numeric num text-warn" } else { "numeric num" }>{event_cost(&event)}</td>
                </tr>
            }
        })
        .collect_view();
    let no_features = data.by_feature.is_empty();
    let no_services = data.by_service.is_empty();
    let no_recent = data.recent.is_empty();
    let unknown = summary.unknown_event_count;
    view! {
        <ReadoutBand cols=4 aria_label="Spend summary" class=Signal::derive(move || Some(if refreshing.get() { "refreshing".to_owned() } else { String::new() }))>
            <Readout label="Range total" value=format_cents(Some(summary.selected_cost_cents)).unwrap_or_default() size=ReadoutSize::Xl tone=Tone::Signal>
                {format!("all time {}", format_cents(Some(summary.all_time_cost_cents)).unwrap_or_default())}
            </Readout>
            <Readout label="Average per day" value=format_cents(Some(summary.average_daily_cost_cents)).unwrap_or_default()/>
            <Readout label="Highest day" value=format_cents(highest.as_ref().map(|d| d.cost_cents)).unwrap_or_else(|| "—".to_owned())>
                {highest.as_ref().map(|d| format_calendar_date(&d.date, true)).unwrap_or_else(|| "No priced usage".to_owned())}
            </Readout>
            <Readout label="Events" value=locale_number(summary.event_count as f64)>
                {format!("{} requests", locale_number(summary.requests))}
            </Readout>
        </ReadoutBand>
        {(unknown > 0).then(|| view! {
            <InlineNote tone=Tone::Warn>{format!("{} unpriced events are excluded from these totals.", locale_number(unknown as f64))}</InlineNote>
        })}
        <Panel title="Daily spend by feature" pad=true refreshing>
            <DailyChart data=data.clone()/>
        </Panel>
        <div class="grid-2">
            <Panel title="By feature" refreshing>
                {if no_features {
                    view! { <EmptyState compact=true message="No feature costs in this range."/> }.into_any()
                } else {
                    view! { <div class="table-wrap"><table class="table"><tbody>{features}</tbody></table></div> }.into_any()
                }}
            </Panel>
            <Panel title="By model" refreshing>
                {if no_services {
                    view! { <EmptyState compact=true message="No service costs in this range."/> }.into_any()
                } else {
                    view! { <div class="table-wrap"><table class="table"><tbody>{services}</tbody></table></div> }.into_any()
                }}
            </Panel>
        </div>
        <Panel title="Recent events" refreshing>
            {if no_recent {
                view! { <EmptyState compact=true message="No recent cost events in this range."/> }.into_any()
            } else {
                view! {
                    <div class="table-wrap">
                        <table class="table dense">
                            <thead><tr><th class="hide-phone">"When"</th><th class="grow">"Operation"</th><th class="hide-phone">"Kind"</th><th class="numeric">"Cost"</th></tr></thead>
                            <tbody>{recent}</tbody>
                        </table>
                    </div>
                }.into_any()
            }}
        </Panel>
    }
}

#[component]
pub fn CostsPage() -> impl IntoView {
    let range = RwSignal::new(CostRange::Days(30));
    let data = RwSignal::new(None::<CostsResponse>);
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(true);
    let displayed_range = RwSignal::new(CostRange::Days(30));
    let retry = RwSignal::new(0u32);

    Effect::new(move |_| {
        let selected = range.get();
        retry.track();
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

    let range_options = Signal::derive(|| {
        RANGES
            .into_iter()
            .map(|r| SegOption::new(r, range_label(r)))
            .collect::<Vec<_>>()
    });
    let title = Signal::derive(move || {
        data.with(|d| {
            d.as_ref().map_or_else(
                || "Costs".to_owned(),
                |d| {
                    format!(
                        "{} over {}.",
                        format_cents(Some(d.summary.selected_cost_cents)).unwrap_or_default(),
                        match displayed_range.get() {
                            CostRange::All => "all time".to_owned(),
                            CostRange::Days(n) => format!("{n} days"),
                        }
                    )
                },
            )
        })
    });
    let refreshing = Signal::derive(move || loading.get() && data.with(Option::is_some));
    view! {
        <PageHead title eyebrow="Costs" sentence=true actions=ViewFn::from(move || view! {
            <Segmented options=range_options value=range on_change=Callback::new(move |r| range.set(r)) aria_label="Cost date range"/>
        })/>
        {move || match (data.with(Option::is_some), error.get()) {
            (false, None) => Some(view! { <SkeletonRows count=8/> }.into_any()),
            (false, Some(e)) => Some(view! {
                <ErrorState title="Costs could not load" raw=e retry=Callback::new(move |()| retry.update(|n| *n += 1)) page=true/>
            }.into_any()),
            (true, Some(e)) => Some(view! {
                <ErrorState title=format!("Could not load that range; showing {} data.", range_text(displayed_range.get())) raw=e retry=Callback::new(move |()| retry.update(|n| *n += 1))/>
            }.into_any()),
            (true, None) => None,
        }}
        {move || data.get().map(|data| view! { <div class="stack-lg"><CostsContent data refreshing/></div> })}
    }
}
