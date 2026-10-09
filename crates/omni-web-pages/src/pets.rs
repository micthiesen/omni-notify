//! Pet weight and litter-box visit trends.

use leptos::html::Div;
use leptos::prelude::*;
use omni_api::pets::{DailyVisit, Pet, PetsResponse, WeightEntry};
use omni_web_kit::api;
use omni_web_kit::charts::{Curve, LineChart, LinePoint, LineSeries};
use omni_web_kit::task::{on_cleanup_local, spawn_scoped};
use omni_web_kit::utils::js::{
    date_locale_date_string, iso_string, js_round, now_ms, number_string, parse_date_ms, to_fixed,
};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

const COLORS: [&str; 6] = [
    "#4fc3f7", "#81c784", "#ffb74d", "#e57373", "#ba68c8", "#4db6ac",
];
const MS_PER_DAY: f64 = 86_400_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    Days7,
    Days30,
    Days90,
    All,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Days7, Range::Days30, Range::Days90, Range::All];

    pub fn days(self) -> Option<i64> {
        match self {
            Range::Days7 => Some(7),
            Range::Days30 => Some(30),
            Range::Days90 => Some(90),
            Range::All => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Range::Days7 => "7d",
            Range::Days30 => "30d",
            Range::Days90 => "90d",
            Range::All => "All",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChartMode {
    Weight,
    Visits,
}

/// The CSV export link (`?days=` unless the whole history is shown).
pub fn export_url(pet_id: &str, range: Range) -> String {
    match range.days() {
        Some(days) => format!("/api/pets/{pet_id}/export.csv?days={days}"),
        None => format!("/api/pets/{pet_id}/export.csv"),
    }
}

/// `new Date()` moved back `days` local calendar days (`setDate`), as epoch ms.
fn cutoff_ms(days: i64) -> f64 {
    let now = js_sys::Date::new_0();
    let day = i32::try_from(i64::from(now.get_date()) - days).unwrap_or(i32::MIN);
    js_sys::Date::new_with_year_month_day_hr_min_sec_milli(
        now.get_full_year(),
        now.get_month() as i32,
        day,
        now.get_hours() as i32,
        now.get_minutes() as i32,
        now.get_seconds() as i32,
        now.get_milliseconds() as i32,
    )
    .get_time()
}

/// Weight readings at or after `cutoff` (epoch ms; `None` keeps all).
pub fn filter_weights(
    history: &[(f64, WeightEntry)],
    cutoff: Option<f64>,
) -> Vec<(f64, WeightEntry)> {
    match cutoff {
        None => history.to_vec(),
        Some(cutoff) => history
            .iter()
            .filter(|(t, _)| *t >= cutoff)
            .cloned()
            .collect(),
    }
}

/// Visit days on or after the UTC date of `cutoff` (`toISOString().slice(0, 10)`).
pub fn filter_visits(visits: &[DailyVisit], cutoff: Option<f64>) -> Vec<DailyVisit> {
    match cutoff {
        None => visits.to_vec(),
        Some(cutoff) => {
            let cutoff_day: String = iso_string(cutoff).chars().take(10).collect();
            visits
                .iter()
                .filter(|v| v.date >= cutoff_day)
                .cloned()
                .collect()
        }
    }
}

/// Exponentially weighted moving average.
pub fn ewma(values: &[f64], alpha: f64) -> Vec<f64> {
    let mut result: Vec<f64> = Vec::with_capacity(values.len());
    for (i, value) in values.iter().enumerate() {
        let next = match i {
            0 => *value,
            _ => alpha * value + (1.0 - alpha) * result[i - 1],
        };
        result.push(next);
    }
    result
}

/// Least-squares fit: `(slope, intercept, r2)`.
pub fn linear_regression(points: &[(f64, f64)]) -> (f64, f64, f64) {
    let n = points.len() as f64;
    if points.len() < 2 {
        return (0.0, points.first().map_or(0.0, |p| p.1), 0.0);
    }
    let (mut sum_x, mut sum_y, mut sum_xy, mut sum_xx) = (0.0, 0.0, 0.0, 0.0);
    for (x, y) in points {
        sum_x += x;
        sum_y += y;
        sum_xy += x * y;
        sum_xx += x * x;
    }
    let denom = n * sum_xx - sum_x * sum_x;
    if denom == 0.0 {
        return (0.0, sum_y / n, 0.0);
    }
    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;
    let mean_y = sum_y / n;
    let (mut ss_res, mut ss_tot) = (0.0, 0.0);
    for (x, y) in points {
        let predicted = intercept + slope * x;
        ss_res += (y - predicted).powi(2);
        ss_tot += (y - mean_y).powi(2);
    }
    let r2 = if ss_tot == 0.0 {
        1.0
    } else {
        1.0 - ss_res / ss_tot
    };
    (slope, intercept, r2)
}

/// One plotted point: raw value, smoothed value and trend line value.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartPoint {
    pub epoch: f64,
    pub value: f64,
    pub smoothed: f64,
    pub trend: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartBuild {
    pub data: Vec<ChartPoint>,
    pub y_domain: (f64, f64),
    pub y_min_zero: bool,
    pub show_brush: bool,
    pub slope_per_week: f64,
    pub r2: f64,
}

fn round2(value: f64) -> f64 {
    js_round(value * 100.0) / 100.0
}

/// `buildChartConfig`: smoothing, trend and y domain for `(epoch, value)` points.
pub fn build_chart(points: &[(f64, f64)], y_min_zero: bool, range: Range) -> ChartBuild {
    let t0 = points.first().map_or(0.0, |p| p.0);
    let values: Vec<f64> = points.iter().map(|p| p.1).collect();
    let regression: Vec<(f64, f64)> = points
        .iter()
        .map(|(t, v)| ((t - t0) / MS_PER_DAY, *v))
        .collect();
    let (slope, intercept, r2) = linear_regression(&regression);
    let smoothed = ewma(&values, 0.15);
    let data: Vec<ChartPoint> = points
        .iter()
        .enumerate()
        .map(|(i, (epoch, value))| ChartPoint {
            epoch: *epoch,
            value: *value,
            smoothed: round2(smoothed[i]),
            trend: round2(intercept + slope * ((epoch - t0) / MS_PER_DAY)),
        })
        .collect();
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let y_domain = if y_min_zero {
        (0.0, if values.is_empty() { 1.0 } else { max } + 1.0)
    } else {
        let (min, max) = if values.is_empty() {
            (0.0, 10.0)
        } else {
            (min, max)
        };
        let padding = ((max - min) * 0.15).max(0.2);
        (
            ((min - padding) * 10.0).floor() / 10.0,
            ((max + padding) * 10.0).ceil() / 10.0,
        )
    };
    ChartBuild {
        show_brush: range == Range::All && data.len() > 60,
        data,
        y_domain,
        y_min_zero,
        slope_per_week: slope * 7.0,
        r2,
    }
}

/// `R² = 0.812 (strong fit)`.
pub fn fit_title(r2: f64) -> String {
    let fit = if r2 >= 0.7 {
        "strong"
    } else if r2 >= 0.3 {
        "moderate"
    } else {
        "weak"
    };
    format!("R² = {} ({fit} fit)", to_fixed(r2, 3))
}

/// Average visits over complete days (today excluded).
pub fn average_per_day(visits: &[DailyVisit], today: &str) -> f64 {
    let complete: Vec<&DailyVisit> = visits.iter().filter(|d| d.date != today).collect();
    if complete.is_empty() {
        0.0
    } else {
        complete.iter().map(|d| f64::from(d.count)).sum::<f64>() / complete.len() as f64
    }
}

fn short_date(ms: f64) -> String {
    date_locale_date_string(ms, &[("month", "short"), ("day", "numeric")])
}

fn tooltip_date(ms: f64) -> String {
    date_locale_date_string(
        ms,
        &[("month", "short"), ("day", "numeric"), ("year", "numeric")],
    )
}

/// A tooltip row's swatch and label.
struct TooltipLine {
    stroke: String,
    width: f64,
    dash: Option<&'static str>,
    opacity: f64,
    label: &'static str,
}

#[component]
fn PetCard(pet: Pet, color_index: usize) -> impl IntoView {
    let range = RwSignal::new(Range::Days30);
    let mode = RwSignal::new(ChartMode::Weight);
    let tooltip_active = RwSignal::new(false);
    let chart_ref = NodeRef::<Div>::new();
    let color = COLORS[color_index % COLORS.len()];

    // A tap outside the chart dismisses a touch tooltip.
    let on_pointer =
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |event: web_sys::PointerEvent| {
            let Some(chart) = chart_ref.get_untracked() else {
                return;
            };
            let chart: &web_sys::Node = chart.as_ref();
            let target = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if !chart.contains(target.as_ref()) {
                tooltip_active.set(false);
            }
        });
    let doc = document();
    let _ =
        doc.add_event_listener_with_callback("pointerdown", on_pointer.as_ref().unchecked_ref());
    on_cleanup_local(move || {
        let _ = doc.remove_event_listener_with_callback(
            "pointerdown",
            on_pointer.as_ref().unchecked_ref(),
        );
    });

    let mut sorted: Vec<(f64, WeightEntry)> = pet
        .weight_history
        .iter()
        .map(|e| (parse_date_ms(&e.timestamp).unwrap_or(f64::NAN), e.clone()))
        .collect();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let daily_visits = pet.daily_visits.clone();

    let filtered_weight =
        Memo::new(move |_| filter_weights(&sorted, range.get().days().map(cutoff_ms)));
    let visits_for_filter = daily_visits.clone();
    let filtered_visits =
        Memo::new(move |_| filter_visits(&visits_for_filter, range.get().days().map(cutoff_ms)));
    let weight_result = Memo::new(move |_| {
        let points: Vec<(f64, f64)> =
            filtered_weight.with(|w| w.iter().map(|(t, e)| (*t, e.weight)).collect());
        build_chart(&points, false, range.get())
    });
    let visit_result = Memo::new(move |_| {
        let points: Vec<(f64, f64)> = filtered_visits.with(|v| {
            v.iter()
                .map(|d| {
                    (
                        parse_date_ms(&d.date).unwrap_or(f64::NAN),
                        f64::from(d.count),
                    )
                })
                .collect()
        });
        build_chart(&points, true, range.get())
    });
    let chart = Memo::new(move |_| match mode.get() {
        ChartMode::Weight => weight_result.get(),
        ChartMode::Visits => visit_result.get(),
    });

    let today: String = iso_string(now_ms()).chars().take(10).collect();
    let visits_today = daily_visits
        .iter()
        .find(|d| d.date == today)
        .map_or(0, |d| d.count);
    let avg_today = today.clone();
    let avg_per_day = move || filtered_visits.with(|v| average_per_day(v, &avg_today));

    let unit = Memo::new(move |_| match mode.get() {
        ChartMode::Weight => "lbs",
        ChartMode::Visits => "visits",
    });
    let label = Memo::new(move |_| match mode.get() {
        ChartMode::Weight => "Weight",
        ChartMode::Visits => "Visits",
    });

    let data = Signal::derive(move || {
        chart.with(|c| {
            c.data
                .iter()
                .map(|p| LinePoint {
                    x: p.epoch,
                    values: vec![Some(p.value), Some(p.smoothed), Some(p.trend)],
                })
                .collect::<Vec<_>>()
        })
    });
    let series = Signal::stored(vec![
        LineSeries {
            key: "value".into(),
            stroke: color.into(),
            width: 2.0,
            dash: None,
            opacity: 1.0,
            dot: true,
            curve: Curve::Monotone,
        },
        LineSeries {
            key: "smoothed".into(),
            stroke: "var(--text)".into(),
            width: 2.0,
            dash: None,
            opacity: 0.6,
            dot: false,
            curve: Curve::Monotone,
        },
        LineSeries {
            key: "trend".into(),
            stroke: color.into(),
            width: 1.5,
            dash: Some("6 4".into()),
            opacity: 0.5,
            dot: false,
            curve: Curve::Linear,
        },
    ]);
    let tooltip = Callback::new(move |index: usize| {
        if !tooltip_active.get_untracked() {
            return ().into_any();
        }
        let Some(point) = chart.with_untracked(|c| c.data.get(index).cloned()) else {
            return ().into_any();
        };
        let unit = unit.get_untracked();
        let lines = [
            (
                point.value,
                TooltipLine {
                    stroke: color.into(),
                    width: 2.0,
                    dash: None,
                    opacity: 1.0,
                    label: label.get_untracked(),
                },
            ),
            (
                point.smoothed,
                TooltipLine {
                    stroke: "var(--text)".into(),
                    width: 2.0,
                    dash: None,
                    opacity: 0.6,
                    label: "Smoothed",
                },
            ),
            (
                point.trend,
                TooltipLine {
                    stroke: color.into(),
                    width: 1.5,
                    dash: Some("4 3"),
                    opacity: 0.5,
                    label: "Trend",
                },
            ),
        ];
        view! {
            <div class="custom-tooltip">
                <div class="tooltip-label">{tooltip_date(point.epoch)}</div>
                {lines
                    .into_iter()
                    .map(|(value, style)| {
                        view! {
                            <div class="tooltip-row">
                                <svg width="20" height="12" class="tooltip-swatch">
                                    <line
                                        x1="0"
                                        y1="6"
                                        x2="20"
                                        y2="6"
                                        stroke=style.stroke
                                        stroke-width=style.width
                                        stroke-dasharray=style.dash
                                        stroke-opacity=style.opacity
                                    ></line>
                                </svg>
                                <span>{format!("{}: {} {unit}", style.label, number_string(value))}</span>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        }
        .into_any()
    });
    let y_tick = Callback::new(move |v: f64| {
        if chart.with_untracked(|c| c.y_min_zero) {
            number_string(js_round(v))
        } else {
            number_string(v)
        }
    });

    let pet_id = pet.pet_id.clone();
    let pet_name = pet.name.clone();
    view! {
        <div class="pet-card">
            <div class="pet-header">
                <div class="meta-row">
                    <span class="pet-name">{pet.name.clone()}</span>
                    <span class="pet-weight">{format!("{} lbs", number_string(pet.current_weight))}</span>
                    {move || {
                        (filtered_weight.with(Vec::len) >= 2)
                            .then(|| {
                                let (slope, r2) = weight_result.with(|w| (w.slope_per_week, w.r2));
                                view! {
                                    <span
                                        class=format!("pet-trend {}", if slope >= 0.0 { "up" } else { "down" })
                                        title=fit_title(r2)
                                    >
                                        {format!(
                                            "{}{} lbs/wk",
                                            if slope >= 0.0 { "+" } else { "" },
                                            to_fixed(slope, 2),
                                        )}
                                    </span>
                                }
                            })
                    }}
                    <span class="pet-visits">{format!("{visits_today} today")}</span>
                    <span class="pet-visits">{move || format!("avg {}/day", to_fixed(avg_per_day(), 1))}</span>
                </div>
                <div class="range-controls">
                    <div class="mode-toggle">
                        <button
                            class=move || format!("range-btn {}", if mode.get() == ChartMode::Weight { "active" } else { "" })
                            aria-pressed=move || (mode.get() == ChartMode::Weight).to_string()
                            on:click=move |_| mode.set(ChartMode::Weight)
                        >
                            "Weight"
                        </button>
                        <button
                            class=move || format!("range-btn {}", if mode.get() == ChartMode::Visits { "active" } else { "" })
                            aria-pressed=move || (mode.get() == ChartMode::Visits).to_string()
                            on:click=move |_| mode.set(ChartMode::Visits)
                        >
                            "Visits"
                        </button>
                    </div>
                    <a
                        href=move || export_url(&pet_id, range.get())
                        class="export-btn"
                        title="Export CSV"
                        aria-label=format!("Export {pet_name} weight data as CSV")
                    >
                        <svg
                            width="16"
                            height="16"
                            viewBox="0 0 16 16"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="1.5"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            aria-hidden="true"
                        >
                            <path d="M8 2v8M5 7l3 3 3-3M3 12h10M3 14h10"></path>
                        </svg>
                    </a>
                    <div class="range-buttons">
                        {Range::ALL
                            .into_iter()
                            .map(|r| {
                                view! {
                                    <button
                                        class=move || format!("range-btn {}", if range.get() == r { "active" } else { "" })
                                        aria-pressed=move || (range.get() == r).to_string()
                                        on:click=move |_| range.set(r)
                                    >
                                        {r.label()}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                </div>
            </div>
            {move || {
                if chart.with(|c| c.data.is_empty()) {
                    let what = if mode.get() == ChartMode::Weight { "weight" } else { "visit" };
                    view! { <div class="no-data">{format!("No {what} data for this range")}</div> }.into_any()
                } else {
                    view! {
                        <div
                            class="chart-container"
                            node_ref=chart_ref
                            on:mousemove=move |_| tooltip_active.set(true)
                            on:mouseleave=move |_| tooltip_active.set(false)
                        >
                            <LineChart
                                data=data
                                series=series
                                y_domain=Signal::derive(move || Some(chart.with(|c| c.y_domain)))
                                x_tick=Callback::new(short_date)
                                y_tick=y_tick
                                tooltip=tooltip
                                show_brush=Signal::derive(move || chart.with(|c| c.show_brush))
                            />
                        </div>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
pub fn PetsPage() -> impl IntoView {
    let pets = RwSignal::new(Vec::<Pet>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);

    spawn_scoped(async move {
        match api::get::<PetsResponse>("/api/pets").await {
            Ok(data) => {
                pets.set(data);
                loading.set(false);
            }
            Err(err) => {
                error.set(Some(err.message().to_owned()));
                loading.set(false);
            }
        }
    });

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"Pet Weight Tracker"</h1>
                <p class="page-subtitle">"Weight trends and daily visit activity."</p>
            </div>
        </div>
        {move || loading.get().then(|| view! { <div class="loading">"Loading..."</div> })}
        {move || {
            error
                .get()
                .map(|e| {
                    view! {
                        <div class="error">
                            <div>"Failed to load pet data"</div>
                            <div class="error-detail">{e}</div>
                        </div>
                    }
                })
        }}
        {move || {
            (!loading.get() && error.with(Option::is_none) && pets.with(Vec::is_empty))
                .then(|| view! { <div class="no-data">"No pets are being tracked yet."</div> })
        }}
        {move || {
            (!loading.get() && error.with(Option::is_none))
                .then(|| {
                    pets.get()
                        .into_iter()
                        .enumerate()
                        .map(|(i, pet)| view! { <PetCard pet=pet color_index=i /> })
                        .collect_view()
                })
        }}
    }
}

#[cfg(test)]
mod tests {
    use omni_api::pets::DailyVisit;

    use super::*;

    #[test]
    fn regression_and_smoothing_match_the_ts_math() {
        let (slope, intercept, r2) = linear_regression(&[(0.0, 1.0), (1.0, 3.0), (2.0, 5.0)]);
        assert!((slope - 2.0).abs() < 1e-12 && (intercept - 1.0).abs() < 1e-12);
        assert!((r2 - 1.0).abs() < 1e-12);
        assert_eq!(linear_regression(&[(0.0, 4.0)]), (0.0, 4.0, 0.0));
        assert_eq!(
            linear_regression(&[(1.0, 4.0), (1.0, 6.0)]),
            (0.0, 5.0, 0.0)
        );
        assert_eq!(ewma(&[10.0, 20.0], 0.15), vec![10.0, 11.5]);
    }

    #[test]
    fn chart_domains_pad_weights_and_zero_base_visits() {
        let day = MS_PER_DAY;
        let weights = build_chart(&[(0.0, 12.0), (day, 12.4)], false, Range::Days30);
        assert_eq!(weights.y_domain, (11.8, 12.6));
        assert!((weights.slope_per_week - 2.8).abs() < 1e-9);
        assert_eq!(weights.data[1].trend, 12.4);
        let visits = build_chart(&[(0.0, 3.0), (day, 5.0)], true, Range::All);
        assert_eq!(visits.y_domain, (0.0, 6.0));
        assert!(!visits.show_brush);
        let empty = build_chart(&[], false, Range::All);
        assert_eq!(empty.y_domain, (-1.5, 11.5));
        let long: Vec<(f64, f64)> = (0..61).map(|i| (f64::from(i) * day, 1.0)).collect();
        assert!(build_chart(&long, true, Range::All).show_brush);
        assert!(!build_chart(&long, true, Range::Days90).show_brush);
    }

    #[test]
    fn export_links_carry_the_range() {
        assert_eq!(
            export_url("PET-1", Range::Days7),
            "/api/pets/PET-1/export.csv?days=7"
        );
        assert_eq!(
            export_url("PET-1", Range::All),
            "/api/pets/PET-1/export.csv"
        );
    }

    #[test]
    fn averages_skip_today_and_visits_filter_by_utc_day() {
        let visits = vec![
            DailyVisit {
                date: "2026-03-01".into(),
                count: 2,
            },
            DailyVisit {
                date: "2026-03-02".into(),
                count: 4,
            },
            DailyVisit {
                date: "2026-03-03".into(),
                count: 9,
            },
        ];
        assert_eq!(average_per_day(&visits, "2026-03-03"), 3.0);
        assert_eq!(average_per_day(&[], "2026-03-03"), 0.0);
        let cutoff = parse_date_ms("2026-03-02T23:00:00.000Z");
        assert_eq!(filter_visits(&visits, cutoff).len(), 2);
        assert_eq!(filter_visits(&visits, None).len(), 3);
        assert_eq!(fit_title(0.5), "R² = 0.500 (moderate fit)");
    }
}
