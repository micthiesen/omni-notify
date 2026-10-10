//! Pet weight and litter-box visit trends: one panel per pet.

use std::collections::HashMap;

use leptos::prelude::*;
use omni_api::pets::{
    DailyVisit, Pet, PetHealthAlertInfo, PetHealthDismissRequest, PetHealthDismissResponse,
    PetHealthFinding, PetHealthKind, PetHealthResponse, PetTrend, PetWeek, PetsResponse,
    WeightEntry,
};
use omni_web_kit::api;
use omni_web_kit::charts::{Curve, LineChart, LinePoint, LineSeries};
use omni_web_kit::components::badges::delta_parts;
use omni_web_kit::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, EmptyState, ErrorState, Icon, Meter, PageHead,
    Panel, Readout, ReadoutSize, SegOption, Segmented, Skeleton, SkeletonKind, Sparkline, Status,
    StatusKind, ToastHandle, ToastKind, Tone, use_toast,
};
use omni_web_kit::feeds::use_task_backed;
use omni_web_kit::hooks::{use_now, use_query_highlight};
use omni_web_kit::task::spawn_detached;
use omni_web_kit::utils::format::{format_absolute, format_relative_at};
use omni_web_kit::utils::js::{
    date_locale_date_string, iso_string, js_round, now_ms, number_string, parse_date_ms, to_fixed,
};

/// The task that reads the scale and evaluates the health rules.
pub const PET_TASK: &str = "PetTracker";
/// The trend card's change windows, in weeks (about 14, 30 and 90 days).
pub const TREND_WEEKS: [u32; 3] = [2, 4, 12];
/// Readings older than this are a gap (the household rule's threshold).
const GAP_HOURS: f64 = 48.0;

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
            Range::Days7 => "7D",
            Range::Days30 => "30D",
            Range::Days90 => "90D",
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

/// Smoothing, trend and y domain for `(epoch, value)` points.
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

/// `+0.12 lbs/wk` (a gain is not an error: no hue either way).
pub fn trend_label(slope_per_week: f64) -> String {
    format!(
        "{}{} lbs/wk",
        if slope_per_week >= 0.0 { "+" } else { "" },
        to_fixed(slope_per_week, 2)
    )
}

/// One tooltip line: swatch style, label and value.
fn tooltip_row(swatch: &'static str, label: &'static str, value: String) -> impl IntoView {
    view! {
        <div class="chart-tooltip-row">
            <i class=format!("chart-tooltip-swatch {swatch}")></i>
            <span>{label}</span>
            <span class="num">{value}</span>
        </div>
    }
}

/// `2 wk` (the change window's label).
pub fn change_label(weeks: u32) -> String {
    format!("{weeks} wk")
}

/// Findings that still need attention (not dismissed for this episode).
pub fn open_findings(findings: &[PetHealthFinding]) -> impl Iterator<Item = &PetHealthFinding> {
    findings.iter().filter(|f| f.dismissed_at.is_none())
}

/// The change windows an undismissed tripped rule covers: the two-week drop
/// rule compares against two and four weeks back; the 90-day rule shows on
/// the 12-week window.
pub fn flagged_weeks(findings: &[PetHealthFinding]) -> Vec<u32> {
    let mut weeks = Vec::new();
    for f in open_findings(findings) {
        match f.kind {
            PetHealthKind::WeightDrop2w => weeks.extend([2, 4]),
            PetHealthKind::WeightDrop90d => weeks.push(12),
            PetHealthKind::VisitDrop | PetHealthKind::DataGap => {}
        }
    }
    weeks
}

/// `(weeks, percent, baseline)` for [`TREND_WEEKS`], in that order.
pub fn shown_changes(trend: &PetTrend) -> Vec<(u32, Option<f64>, Option<f64>)> {
    TREND_WEEKS
        .iter()
        .map(|w| {
            trend
                .changes
                .iter()
                .find(|c| c.weeks == *w)
                .map_or((*w, None, None), |c| (*w, c.percent, c.baseline_weight))
        })
        .collect()
}

/// A rule in words.
pub fn finding_title(kind: PetHealthKind) -> &'static str {
    match kind {
        PetHealthKind::WeightDrop2w => "Weight down over two weeks",
        PetHealthKind::WeightDrop90d => "Weight down over 90 days",
        PetHealthKind::VisitDrop => "Fewer litter-box visits",
        PetHealthKind::DataGap => "No scale readings",
    }
}

/// The durable alert of `pet_id` and `kind` (household rules have no pet).
pub fn alert_for<'a>(
    alerts: &'a [PetHealthAlertInfo],
    pet_id: Option<&str>,
    kind: PetHealthKind,
) -> Option<&'a PetHealthAlertInfo> {
    alerts
        .iter()
        .find(|a| a.kind == kind && a.pet_id.as_deref() == pet_id)
}

/// Weekly medians with readings, oldest first (the sparkline).
pub fn spark_points(weekly: &[PetWeek]) -> Vec<f64> {
    weekly.iter().filter_map(|w| w.median_weight).collect()
}

/// Hours since an ISO reading time.
pub fn hours_since(iso: Option<&str>, now: f64) -> Option<f64> {
    iso.and_then(parse_date_ms).map(|t| (now - t) / 3_600_000.0)
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Headline and lede for the page: steady, a look needed, or a quiet scale.
pub fn pets_sentence(health: &PetHealthResponse, now: f64) -> (String, String) {
    let names: Vec<String> = health.pets.iter().map(|p| p.name.clone()).collect();
    if names.is_empty() {
        return ("No pets tracked yet.".to_owned(), String::new());
    }
    let last = hours_since(health.latest_reading_at.as_deref(), now);
    if health.data_gap.is_some() {
        let hours = last.map_or_else(|| "a while".to_owned(), |h| format!("{} h", js_round(h)));
        return (
            "The scale has gone quiet.".to_owned(),
            format!("No readings for {hours}; trends stop at the last reading."),
        );
    }
    let flagged: Vec<&PetTrend> = health
        .pets
        .iter()
        .filter(|p| open_findings(&p.findings).next().is_some())
        .collect();
    let reading = last.map(|h| {
        if h < 1.0 {
            "Last reading under an hour ago.".to_owned()
        } else {
            format!("Last reading {} h ago.", js_round(h))
        }
    });
    let reading = reading.map(|r| format!(" {r}")).unwrap_or_default();
    if flagged.is_empty() {
        let dismissed: Vec<String> = health
            .pets
            .iter()
            .flat_map(|p| {
                p.findings
                    .iter()
                    .map(move |f| format!("{}: {}", p.name, finding_title(f.kind).to_lowercase()))
            })
            .collect();
        if !dismissed.is_empty() {
            return (
                "Nothing new needs a look.".to_owned(),
                format!("Dismissed: {}.{reading}", dismissed.join("; ")),
            );
        }
        let head = match names.len() {
            1 => format!("{} is steady.", names[0]),
            2 => "Both pets are steady.".to_owned(),
            n => format!("All {n} pets are steady."),
        };
        return (head, format!("No weight or visit alerts.{reading}"));
    }
    let flagged_names: Vec<String> = flagged.iter().map(|p| p.name.clone()).collect();
    let verb = if flagged.len() > 1 { "need" } else { "needs" };
    let head = format!("{} {verb} a look.", join_names(&flagged_names));
    let lede = flagged
        .iter()
        .flat_map(|p| {
            open_findings(&p.findings)
                .map(move |f| format!("{}: {}", p.name, finding_title(f.kind).to_lowercase()))
        })
        .collect::<Vec<_>>()
        .join("; ")
        + ".";
    (head, lede)
}

/// A change cell: `↓3.0%` with the baseline under it; warn only when a rule
/// tripped on that window.
#[component]
fn ChangeCell(
    weeks: u32,
    percent: Option<f64>,
    baseline: Option<f64>,
    flagged: bool,
) -> impl IntoView {
    let (class, text) = match percent {
        Some(p) => {
            let (dir, text) = delta_parts(p);
            let dir = match dir {
                omni_web_kit::components::badges::DeltaDirection::Up => "up",
                omni_web_kit::components::badges::DeltaDirection::Down => "down",
                omni_web_kit::components::badges::DeltaDirection::Flat => "",
            };
            (
                format!("delta {dir}{}", if flagged { " warn" } else { "" }),
                text,
            )
        }
        None => ("delta off".to_owned(), "—".to_owned()),
    };
    let title = match (percent, baseline) {
        (Some(_), Some(b)) => format!(
            "Against the 7 days ending {weeks} weeks ago ({} lb)",
            number_string(b)
        ),
        _ => "Fewer than three readings in one of the windows".to_owned(),
    };
    view! {
        <div class="pet-change" title=title>
            <span class="readout-k">{change_label(weeks)}</span>
            <span class=class>{text}</span>
            <span class="small muted num">{baseline.map_or_else(|| "not enough data".to_owned(), |b| format!("from {}", number_string(b)))}</span>
        </div>
    }
}

/// Dismisses (`dismiss`) or restores a finding, then reloads the health data.
fn set_dismissed(
    pet_id: String,
    kind: PetHealthKind,
    dismiss: bool,
    busy: RwSignal<bool>,
    on_change: Option<Callback<()>>,
    toast: ToastHandle,
) {
    busy.set(true);
    spawn_detached(async move {
        let path = if dismiss {
            "/api/pets/health/dismiss"
        } else {
            "/api/pets/health/restore"
        };
        let body = PetHealthDismissRequest { pet_id, kind };
        match api::post::<PetHealthDismissResponse, _>(path, Some(&body)).await {
            Ok(_) => {
                if let Some(on_change) = on_change {
                    on_change.run(());
                }
            }
            Err(e) => toast.show(
                format!(
                    "Could not {} the alert: {}",
                    if dismiss { "dismiss" } else { "restore" },
                    e.message()
                ),
                ToastKind::Error,
            ),
        }
        busy.try_set(false);
    });
}

/// One tripped rule: warn with a Dismiss button, or quiet once dismissed for
/// this episode with a Restore button.
#[component]
fn FindingRow(
    pet_id: String,
    finding: PetHealthFinding,
    alert: Option<PetHealthAlertInfo>,
    on_change: Option<Callback<()>>,
) -> impl IntoView {
    let now = use_now(60_000);
    let busy = RwSignal::new(false);
    let toast = use_toast();
    let kind = finding.kind;
    let title = finding_title(kind);
    let notified = alert
        .as_ref()
        .and_then(|a| a.last_notified_at.as_deref().and_then(parse_date_ms));
    let dismissed = finding.dismissed_at.as_deref().and_then(parse_date_ms);
    let is_dismissed = finding.dismissed_at.is_some();
    let toggle =
        move |_| set_dismissed(pet_id.clone(), kind, !is_dismissed, busy, on_change, toast);
    view! {
        <div class=if is_dismissed { "row pet-finding dismissed" } else { "row pet-finding" }>
            {if is_dismissed {
                view! { <Status kind=StatusKind::Idle label=format!("{title} · dismissed")/> }.into_any()
            } else {
                view! { <Status kind=StatusKind::Warn label=title/> }.into_any()
            }}
            <span class="row-main"><span class="row-sub">{finding.message.clone()}</span></span>
            <span class="row-end small muted num">
                {move || match (dismissed, notified) {
                    (Some(t), _) => format!("dismissed {}", format_relative_at(t, now.get())),
                    (None, Some(t)) => format!("notified {}", format_relative_at(t, now.get())),
                    (None, None) => "not notified yet".to_owned(),
                }}
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    busy=busy
                    title=if is_dismissed {
                        "Show this alert as needing attention again".to_owned()
                    } else {
                        "Hide this alert until a new episode or a repeat push".to_owned()
                    }
                    aria_label=format!("{} {}", if is_dismissed { "Restore" } else { "Dismiss" }, title.to_lowercase())
                    on_click=Callback::new(toggle)
                >
                    {if is_dismissed { "Restore" } else { "Dismiss" }}
                </Button>
            </span>
        </div>
    }
}

/// The trend card: robust weight with its 26-week sparkline, 2/4/12-week
/// changes, visit frequency, reading age and the active alerts.
#[component]
fn PetTrendBlock(
    trend: PetTrend,
    alerts: Vec<PetHealthAlertInfo>,
    household_gap: bool,
    on_change: Option<Callback<()>>,
) -> impl IntoView {
    let now = use_now(60_000);
    let flagged = flagged_weeks(&trend.findings);
    let points = spark_points(&trend.weekly);
    let weeks_shown = points.len();
    let visit_flag = open_findings(&trend.findings).any(|f| f.kind == PetHealthKind::VisitDrop);
    let latest = trend.latest_reading_at.clone();
    let latest_ms = latest.as_deref().and_then(parse_date_ms);
    let name = trend.name.clone();
    let findings = trend.findings.clone();
    let pet_id = trend.pet_id.clone();
    let usual = trend.usual_visits_per_week;
    let visits = trend.visits_last_7_days;
    let weight = trend.weight;
    view! {
        <div class="pet-trend">
            <div class="pet-trend-weight">
                <Readout
                    label="Weight · 7-day median"
                    value=weight.map_or_else(|| "—".to_owned(), number_string)
                    unit="lb"
                    stale=Signal::derive(move || latest_ms.is_some_and(|t| now.get() - t > GAP_HOURS * 3_600_000.0))
                >
                    {move || match latest_ms {
                        Some(t) => {
                            let gap = now.get() - t > GAP_HOURS * 3_600_000.0 || household_gap;
                            view! {
                                <span class=if gap { "text-warn" } else { "" } title=format_absolute(t)>
                                    {format!("last reading {}", format_relative_at(t, now.get()))}
                                </span>
                            }.into_any()
                        }
                        None => view! { <span class="text-warn">"No readings yet"</span> }.into_any(),
                    }}
                </Readout>
                {(weeks_shown > 1).then(|| view! {
                    <div class="pet-spark">
                        <Sparkline points=points label=format!("{name}: weekly median weight, last {weeks_shown} weeks") height=44/>
                        <span class="small muted">{format!("{weeks_shown} weeks")}</span>
                    </div>
                })}
            </div>
            <div class="pet-changes" role="group" aria-label="Weight change">
                {shown_changes(&trend).into_iter().map(|(weeks, percent, baseline)| view! {
                    <ChangeCell weeks percent baseline flagged=flagged.contains(&weeks)/>
                }).collect_view()}
            </div>
            <div class="pet-visits">
                <Readout
                    label="Visits · 7 days"
                    value=visits.to_string()
                    size=ReadoutSize::M
                    tone=if visit_flag { Tone::Warn } else { Tone::Neutral }
                >
                    {usual.map(|u| view! {
                        <Meter
                            value=f64::from(visits)
                            max=(u * 1.5).max(f64::from(visits)).max(1.0)
                            reference=Some(u)
                            tone=if visit_flag { Tone::Warn } else { Tone::Neutral }
                            width=96
                            label=format!("{visits} visits against a usual {} per week", number_string(js_round(u)))
                        />
                        <span>{format!("usually {}/wk", number_string(js_round(u)))}</span>
                    })}
                </Readout>
            </div>
            {(!findings.is_empty()).then(|| view! {
                <div class="rows pet-findings">
                    {findings.into_iter().map(|finding| {
                        let alert = alert_for(&alerts, Some(&pet_id), finding.kind).cloned();
                        view! { <FindingRow pet_id=pet_id.clone() finding alert on_change/> }
                    }).collect_view()}
                </div>
            })}
        </div>
    }
}

#[component]
fn PetPanel(
    pet: Pet,
    #[prop(optional)] health_trend: Option<PetTrend>,
    #[prop(optional)] alerts: Vec<PetHealthAlertInfo>,
    #[prop(optional)] household_gap: bool,
    /// Reloads the health data after a finding is dismissed or restored.
    #[prop(optional)]
    on_health_change: Option<Callback<()>>,
    #[prop(optional)] highlighted: bool,
    /// Owned by the page so the selection survives data reloads.
    range: RwSignal<Range>,
    mode: RwSignal<ChartMode>,
) -> impl IntoView {
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
            stroke: "var(--signal)".into(),
            width: 2.0,
            dash: None,
            opacity: 1.0,
            dot: true,
            curve: Curve::Monotone,
        },
        LineSeries {
            key: "smoothed".into(),
            stroke: "var(--text-2)".into(),
            width: 1.5,
            dash: None,
            opacity: 0.7,
            dot: false,
            curve: Curve::Monotone,
        },
        LineSeries {
            key: "trend".into(),
            stroke: "var(--text-3)".into(),
            width: 1.5,
            dash: Some("6 4".into()),
            opacity: 0.8,
            dot: false,
            curve: Curve::Linear,
        },
    ]);
    let tooltip = Callback::new(move |index: usize| {
        let Some(point) = chart.with_untracked(|c| c.data.get(index).cloned()) else {
            return ().into_any();
        };
        let unit = unit.get_untracked();
        let value = |v: f64| format!("{} {unit}", number_string(v));
        view! {
            <div class="chart-tooltip-label">{tooltip_date(point.epoch)}</div>
            {tooltip_row("pet-swatch-value", label.get_untracked(), value(point.value))}
            {tooltip_row("pet-swatch-smooth", "Smoothed", value(point.smoothed))}
            {tooltip_row("pet-swatch-trend", "Trend", value(point.trend))}
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
    let chart_label = pet.name.clone();
    let trend = Memo::new(move |_| {
        (filtered_weight.with(Vec::len) >= 2)
            .then(|| weight_result.with(|w| (w.slope_per_week, w.r2)))
    });
    let mode_options = Signal::stored(vec![
        SegOption::new(ChartMode::Weight, "Weight"),
        SegOption::new(ChartMode::Visits, "Visits"),
    ]);
    let range_options = Signal::stored(
        Range::ALL
            .into_iter()
            .map(|r| SegOption::new(r, r.label()))
            .collect::<Vec<_>>(),
    );
    view! {
        <Panel
            class=if highlighted { "pet deep-link-target" } else { "pet" }
            id=format!("pet-{}", pet.pet_id)
            aria_label=pet.name.clone()
        >
            <div class="pet-head">
                <h2 class="pet-name">{pet.name.clone()}</h2>
                <span class="spacer"></span>
                {move || {
                    let href = export_url(&pet_id, range.get());
                    view! {
                        <ButtonLink
                            to=href
                            download=true
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Sm
                            icon=Icon::Download
                            title="Export the selected range as CSV"
                            aria_label=format!("Export {pet_name} weight data as CSV")
                        >
                            "CSV"
                        </ButtonLink>
                    }
                }}
            </div>
            {match health_trend {
                Some(trend) => view! { <PetTrendBlock trend alerts household_gap on_change=on_health_change/> }.into_any(),
                None => view! {
            <div class="pet-figures">
                <Readout label="Weight" value=number_string(pet.current_weight) unit="lbs">
                    {move || {
                        match trend.get() {
                            Some((slope, r2)) => view! {
                                <span class="mono" title=fit_title(r2)>{trend_label(slope)}</span>
                            }
                                .into_any(),
                            None => view! { <span>"Not enough readings for a trend"</span> }.into_any(),
                        }
                    }}
                </Readout>
                <Readout label="Visits today" value=visits_today.to_string()>
                    <span class="mono">{move || format!("avg {}/day", to_fixed(avg_per_day(), 1))}</span>
                </Readout>
            </div>
                }.into_any(),
            }}
            <div class="pet-controls">
                <Segmented
                    options=mode_options
                    value=mode
                    on_change=Callback::new(move |m| mode.set(m))
                    aria_label="Chart"
                    small=true
                />
                <Segmented
                    options=range_options
                    value=range
                    on_change=Callback::new(move |r| range.set(r))
                    aria_label="Range"
                    small=true
                />
            </div>
            {move || {
                if chart.with(|c| c.data.is_empty()) {
                    let what = if mode.get() == ChartMode::Weight { "weight" } else { "visit" };
                    view! {
                        <div class="pet-chart">
                            <EmptyState compact=true message=format!("No {what} data for this range.") />
                        </div>
                    }
                        .into_any()
                } else {
                    let name = chart_label.clone();
                    view! {
                        <div class="pet-chart">
                            <div class="chart-container">
                                <LineChart
                                    data=data
                                    series=series
                                    y_domain=Signal::derive(move || Some(chart.with(|c| c.y_domain)))
                                    x_tick=Callback::new(short_date)
                                    y_tick=y_tick
                                    tooltip=tooltip
                                    show_brush=Signal::derive(move || chart.with(|c| c.show_brush))
                                    label=Signal::derive(move || {
                                        Some(format!("{name} {} over the selected range", label.get().to_lowercase()))
                                    })
                                />
                            </div>
                            <div class="chart-legend">
                                <span class="chart-legend-item"><i class="chart-tooltip-swatch pet-swatch-value"></i>{move || label.get()}</span>
                                <span class="chart-legend-item"><i class="chart-tooltip-swatch pet-swatch-smooth"></i>"Smoothed"</span>
                                <span class="chart-legend-item"><i class="chart-tooltip-swatch pet-swatch-trend"></i>"Trend"</span>
                            </div>
                        </div>
                    }
                        .into_any()
                }
            }}
        </Panel>
    }
}

#[component]
pub fn PetsPage() -> impl IntoView {
    // Both reload whenever the scale task finishes a run.
    let pets = use_task_backed(PET_TASK, || api::get::<PetsResponse>("/api/pets"));
    let health = use_task_backed(PET_TASK, api::fetch_pet_health);
    let now = use_now(60_000);
    let loaded = Signal::derive(move || pets.data.with(Option::is_some));
    let highlighted = use_query_highlight("pet", "pet", loaded);

    let sentence = Memo::new(move |_| {
        health
            .data
            .with(|h| h.as_ref().map(|h| pets_sentence(h, now.get())))
    });
    let title = Signal::derive(move || {
        sentence
            .get()
            .map_or_else(|| "Pets".to_owned(), |(head, _)| head)
    });
    let lede = Signal::derive(move || sentence.get().map(|(_, lede)| lede));

    // The panel inputs, compared by value: a reload that returns the same
    // data (or only fails) does not rebuild the panels.
    let body = Memo::new(move |_| match (pets.data.get(), pets.error.get()) {
        (None, Some(e)) => PetsBody::Error(e),
        (None, None) => PetsBody::Loading,
        (Some(list), _) if list.is_empty() => PetsBody::Empty,
        (Some(list), _) => {
            let (trends, alerts, gap) = health.data.with(|h| match h {
                Some(h) => (h.pets.clone(), h.alerts.clone(), h.data_gap.is_some()),
                None => (Vec::new(), Vec::new(), false),
            });
            PetsBody::Grid {
                pets: list,
                trends,
                alerts,
                gap,
            }
        }
    });
    // Each pet's chart controls, created under the page's owner so they
    // outlive a rebuild of the grid.
    let owner = Owner::current();
    let reload_health = Callback::new(move |()| health.reload());
    let controls =
        StoredValue::new(HashMap::<String, (RwSignal<Range>, RwSignal<ChartMode>)>::new());
    let controls_for = move |pet_id: &str| {
        if let Some(found) = controls.with_value(|m| m.get(pet_id).copied()) {
            return found;
        }
        let make = || {
            (
                RwSignal::new(Range::Days30),
                RwSignal::new(ChartMode::Weight),
            )
        };
        let created = owner.as_ref().map_or_else(make, |o| o.with(make));
        controls.update_value(|m| {
            m.insert(pet_id.to_owned(), created);
        });
        created
    };

    view! {
        <PageHead title lede sentence=true/>
        {move || health.error.get().filter(|_| health.data.with(Option::is_none)).map(|e| view! {
            <ErrorState
                warn=true
                title="Health trends could not load"
                detail="Weight charts still work; alerts and trend cards are missing."
                raw=e
                retry=Callback::new(move |()| health.reload())
            />
        })}
        {move || {
            match body.get() {
                PetsBody::Error(e) => view! {
                    <ErrorState
                        title="Pet data could not load"
                        raw=e
                        retry=Callback::new(move |()| pets.reload())
                        page=true
                    />
                }
                .into_any(),
                PetsBody::Loading => view! {
                    <div class="pets-grid" aria-busy="true">
                        {(0..2)
                            .map(|_| view! {
                                <div class="panel pet">
                                    <div class="pet-head"><Skeleton kind=SkeletonKind::Title width="30%" /></div>
                                    <div class="pet-figures"><Skeleton kind=SkeletonKind::Readout /><Skeleton kind=SkeletonKind::Readout /></div>
                                    <div class="pet-chart"><div class="chart-container"></div></div>
                                </div>
                            })
                            .collect_view()}
                    </div>
                }
                .into_any(),
                PetsBody::Empty => {
                    view! { <EmptyState message="No pets are being tracked yet." icon=Icon::Paw /> }.into_any()
                }
                PetsBody::Grid { pets: list, trends, alerts, gap } => {
                    let highlighted = highlighted.clone();
                    view! {
                        <div class="pets-grid">
                            {list.into_iter().map(|pet| {
                                let trend = trends.iter().find(|t| t.pet_id == pet.pet_id).cloned();
                                let hl = highlighted.as_deref() == Some(pet.pet_id.as_str());
                                let alerts = alerts.clone();
                                let (range, mode) = controls_for(&pet.pet_id);
                                match trend {
                                    Some(trend) => view! { <PetPanel pet health_trend=trend alerts household_gap=gap on_health_change=reload_health highlighted=hl range mode/> }.into_any(),
                                    None => view! { <PetPanel pet highlighted=hl range mode/> }.into_any(),
                                }
                            }).collect_view()}
                        </div>
                    }
                    .into_any()
                }
            }
        }}
    }
}

/// What the pets grid shows.
#[derive(Clone, Debug, PartialEq)]
enum PetsBody {
    Error(String),
    Loading,
    Empty,
    Grid {
        pets: Vec<Pet>,
        trends: Vec<PetTrend>,
        alerts: Vec<PetHealthAlertInfo>,
        gap: bool,
    },
}

/// Pets with an undismissed tripped rule (and the household data gap), for Home.
pub fn attention_count(health: &PetHealthResponse) -> usize {
    health
        .pets
        .iter()
        .filter(|p| open_findings(&p.findings).next().is_some())
        .count()
        + usize::from(health.data_gap.is_some())
}

#[cfg(test)]
mod tests {
    use omni_api::pets::DailyVisit;

    use super::*;

    #[test]
    fn regression_and_smoothing_match_pinned_values() {
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

    fn trend(name: &str, findings: Vec<PetHealthKind>) -> PetTrend {
        PetTrend {
            pet_id: format!("PET-{name}"),
            name: name.into(),
            weight: Some(12.45),
            latest_reading_at: Some("2026-10-09T10:00:00.000Z".into()),
            weekly: vec![
                PetWeek {
                    start: String::new(),
                    end: String::new(),
                    readings: 9,
                    median_weight: Some(12.8),
                },
                PetWeek {
                    start: String::new(),
                    end: String::new(),
                    readings: 0,
                    median_weight: None,
                },
                PetWeek {
                    start: String::new(),
                    end: String::new(),
                    readings: 7,
                    median_weight: Some(12.45),
                },
            ],
            changes: vec![
                omni_api::pets::PetWeightChange {
                    weeks: 4,
                    percent: Some(-1.2),
                    baseline_weight: Some(12.6),
                },
                omni_api::pets::PetWeightChange {
                    weeks: 2,
                    percent: Some(-3.03),
                    baseline_weight: Some(12.84),
                },
                omni_api::pets::PetWeightChange {
                    weeks: 26,
                    percent: Some(1.0),
                    baseline_weight: Some(12.3),
                },
            ],
            visits_last_7_days: 9,
            usual_visits_per_week: Some(22.5),
            findings: findings
                .into_iter()
                .map(|kind| PetHealthFinding {
                    kind,
                    value: 3.0,
                    message: "m".into(),
                    dismissed_at: None,
                })
                .collect(),
        }
    }

    fn health(pets: Vec<PetTrend>, gap: bool) -> PetHealthResponse {
        PetHealthResponse {
            generated_at: String::new(),
            latest_reading_at: Some("2026-10-09T10:00:00.000Z".into()),
            hours_since_latest_reading: Some(2.0),
            data_gap: gap.then(|| PetHealthFinding {
                kind: PetHealthKind::DataGap,
                value: 52.0,
                message: "gap".into(),
                dismissed_at: None,
            }),
            pets,
            alerts: Vec::new(),
        }
    }

    #[test]
    fn trend_cards_pick_windows_and_flag_tripped_rules() {
        let t = trend("Sandy", vec![PetHealthKind::WeightDrop2w]);
        assert_eq!(
            shown_changes(&t),
            vec![
                (2, Some(-3.03), Some(12.84)),
                (4, Some(-1.2), Some(12.6)),
                (12, None, None)
            ]
        );
        assert_eq!(flagged_weeks(&t.findings), vec![2, 4]);
        assert_eq!(spark_points(&t.weekly), vec![12.8, 12.45]);
        assert_eq!(change_label(12), "12 wk");
        let alerts = vec![PetHealthAlertInfo {
            pet_id: Some("PET-Sandy".into()),
            kind: PetHealthKind::WeightDrop2w,
            active: true,
            last_notified_at: None,
            last_message: None,
            recovered_at: None,
        }];
        assert!(alert_for(&alerts, Some("PET-Sandy"), PetHealthKind::WeightDrop2w).is_some());
        assert!(alert_for(&alerts, None, PetHealthKind::WeightDrop2w).is_none());
    }

    #[test]
    fn sentences_read_steady_flagged_or_quiet() {
        let now = parse_date_ms("2026-10-09T12:00:00.000Z").unwrap();
        let steady = health(vec![trend("Sandy", vec![]), trend("Mochi", vec![])], false);
        assert_eq!(
            pets_sentence(&steady, now),
            (
                "Both pets are steady.".into(),
                "No weight or visit alerts. Last reading 2 h ago.".into()
            )
        );
        let flagged = health(
            vec![
                trend("Sandy", vec![PetHealthKind::VisitDrop]),
                trend("Mochi", vec![]),
            ],
            false,
        );
        assert_eq!(
            pets_sentence(&flagged, now),
            (
                "Sandy needs a look.".into(),
                "Sandy: fewer litter-box visits.".into()
            )
        );
        assert_eq!(attention_count(&flagged), 1);
        let quiet = health(vec![trend("Sandy", vec![])], true);
        assert_eq!(pets_sentence(&quiet, now).0, "The scale has gone quiet.");
        assert_eq!(attention_count(&quiet), 1);
    }

    #[test]
    fn dismissed_findings_leave_the_attention_views() {
        let now = parse_date_ms("2026-10-09T12:00:00.000Z").unwrap();
        let mut sandy = trend(
            "Sandy",
            vec![PetHealthKind::WeightDrop90d, PetHealthKind::VisitDrop],
        );
        sandy.findings[0].dismissed_at = Some("2026-10-09T11:00:00.000Z".into());
        let partly = health(vec![sandy.clone(), trend("Mochi", vec![])], false);
        assert_eq!(attention_count(&partly), 1);
        assert_eq!(
            pets_sentence(&partly, now),
            (
                "Sandy needs a look.".into(),
                "Sandy: fewer litter-box visits.".into()
            )
        );
        assert_eq!(flagged_weeks(&sandy.findings), Vec::<u32>::new());

        sandy.findings[1].dismissed_at = Some("2026-10-09T11:00:00.000Z".into());
        let all = health(vec![sandy, trend("Mochi", vec![])], false);
        assert_eq!(attention_count(&all), 0);
        assert_eq!(
            pets_sentence(&all, now),
            (
                "Nothing new needs a look.".into(),
                "Dismissed: Sandy: weight down over 90 days; Sandy: fewer litter-box visits. Last reading 2 h ago.".into()
            )
        );
        // The data gap cannot be dismissed and always counts.
        let gap = health(vec![trend("Sandy", vec![])], true);
        assert_eq!(attention_count(&gap), 1);
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
        assert_eq!(trend_label(0.123), "+0.12 lbs/wk");
        assert_eq!(trend_label(-0.5), "-0.50 lbs/wk");
    }
}
