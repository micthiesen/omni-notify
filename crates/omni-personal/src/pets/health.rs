//! Pet health watch: pure trend computation and rule evaluation over the
//! stored scale readings.
//!
//! All windows are rolling and end at `now`. Weights are robust medians: the
//! Litter-Robot attributes visits by weight and the cats are about a pound
//! apart, so a window drops readings more than [`OUTLIER_LB`] from its own
//! median before taking the median again. Thresholds were calibrated by
//! replaying the production history hourly (see `docs/pet-health.md`).

use omni_api::pets::{
    PetHealthAlertInfo, PetHealthFinding, PetHealthKind, PetHealthResponse, PetTrend, PetWeek,
    PetWeightChange,
};
use omni_core::js::{math_round, to_iso_string};

use super::persistence::{PetWithHistory, WeightHistoryRow};

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 24 * HOUR_MS;
pub const WEEK_MS: i64 = 7 * DAY_MS;

/// Readings further than this from their window's median are attributed to
/// the other cat and ignored.
pub const OUTLIER_LB: f64 = 1.0;
/// Readings each window needs before a weight rule may trip or clear.
pub const MIN_RULE_READINGS: usize = 8;
/// Readings each window needs for a displayed change.
pub const MIN_DISPLAY_READINGS: usize = 3;

/// `weight-drop-2w` trips at this drop over 14 days...
pub const DROP_2W_PERCENT: f64 = 3.0;
/// ...when the 28-day drop confirms the decline is not a dip and rebound.
pub const DROP_2W_CONFIRM_PERCENT: f64 = 2.0;
/// It clears below these.
pub const DROP_2W_CLEAR_PERCENT: f64 = 2.0;
pub const DROP_2W_CONFIRM_CLEAR_PERCENT: f64 = 1.0;
/// `weight-drop-90d` trips at and clears below.
pub const DROP_90D_PERCENT: f64 = 5.0;
pub const DROP_90D_CLEAR_PERCENT: f64 = 4.0;
/// `visit-drop` trips at and clears above these ratios of the usual week.
pub const VISIT_DROP_RATIO: f64 = 0.5;
pub const VISIT_CLEAR_RATIO: f64 = 0.75;
/// Usual weeks below this many visits are too sparse to judge.
pub const MIN_USUAL_VISITS: f64 = 7.0;
/// Preceding 7-day blocks the usual visit count is the median of.
pub const VISIT_BASELINE_WEEKS: i64 = 8;
/// `data-gap` trips when no pet has a reading for this long.
pub const GAP_MS: i64 = 48 * HOUR_MS;
/// Visit counts are judged only when the household has no silence longer than
/// this in the last 7 days (an outage is the gap rule's business).
pub const COVERAGE_MAX_SILENCE_MS: i64 = 24 * HOUR_MS;
/// Displayed weight changes.
pub const CHANGE_WEEKS: [u32; 4] = [2, 4, 12, 26];

/// One parsed scale reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub at_ms: i64,
    pub weight: f64,
}

/// Whisker timestamps carry no offset but are UTC (a reading synced at
/// `04:40Z` is stamped `04:27:58`); an explicit offset is honored.
pub fn reading_ms(timestamp: &str) -> Option<i64> {
    if let Ok(at) = timestamp.parse::<jiff::Timestamp>() {
        return Some(at.as_millisecond());
    }
    let civil = timestamp.parse::<jiff::civil::DateTime>().ok()?;
    let zoned = civil.to_zoned(jiff::tz::TimeZone::UTC).ok()?;
    Some(zoned.timestamp().as_millisecond())
}

/// Parsed readings, oldest first; unparsable or non-finite rows are skipped.
pub fn readings(rows: &[WeightHistoryRow]) -> Vec<Reading> {
    let mut out: Vec<Reading> = rows
        .iter()
        .filter(|row| row.weight.is_finite())
        .filter_map(|row| {
            reading_ms(&row.timestamp).map(|at_ms| Reading {
                at_ms,
                weight: row.weight,
            })
        })
        .collect();
    out.sort_by_key(|r| r.at_ms);
    out
}

/// `Math.round(n * 100) / 100`.
pub fn round2(n: f64) -> f64 {
    math_round(n * 100.0) / 100.0
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        f64::midpoint(values[mid - 1], values[mid])
    } else {
        values[mid]
    })
}

/// A window's robust median and the readings it kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowStat {
    pub median: f64,
    pub count: usize,
}

/// Readings in `(from, to]`.
fn window(readings: &[Reading], from: i64, to: i64) -> impl Iterator<Item = &Reading> {
    readings
        .iter()
        .filter(move |r| r.at_ms > from && r.at_ms <= to)
}

/// The median of `(from, to]` after dropping readings over [`OUTLIER_LB`]
/// from the raw median.
pub fn window_stat(readings: &[Reading], from: i64, to: i64) -> Option<WindowStat> {
    let mut weights: Vec<f64> = window(readings, from, to).map(|r| r.weight).collect();
    let raw = median(&mut weights)?;
    let mut kept: Vec<f64> = weights
        .into_iter()
        .filter(|w| (w - raw).abs() <= OUTLIER_LB)
        .collect();
    let count = kept.len();
    median(&mut kept).map(|median| WindowStat { median, count })
}

/// The last 7 days against the 7 days ending `lag_ms` earlier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeightChange {
    /// Negative is a loss.
    pub percent: f64,
    pub recent: WindowStat,
    pub baseline: WindowStat,
}

impl WeightChange {
    /// Percent lost (positive for a loss).
    pub fn drop(&self) -> f64 {
        -self.percent
    }
}

pub fn weight_change(
    readings: &[Reading],
    now: i64,
    lag_ms: i64,
    min_readings: usize,
) -> Option<WeightChange> {
    let recent = window_stat(readings, now - WEEK_MS, now)?;
    let baseline = window_stat(readings, now - lag_ms - WEEK_MS, now - lag_ms)?;
    if recent.count < min_readings || baseline.count < min_readings || baseline.median <= 0.0 {
        return None;
    }
    Some(WeightChange {
        percent: (recent.median - baseline.median) / baseline.median * 100.0,
        recent,
        baseline,
    })
}

/// Visits in the last 7 days and the median of the preceding blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisitRate {
    pub last_7_days: u32,
    pub usual: Option<f64>,
}

pub fn visit_rate(readings: &[Reading], now: i64) -> VisitRate {
    let count = |from: i64, to: i64| -> u32 {
        u32::try_from(window(readings, from, to).count()).unwrap_or(u32::MAX)
    };
    let mut weeks: Vec<f64> = (1..=VISIT_BASELINE_WEEKS)
        .map(|k| f64::from(count(now - (k + 1) * WEEK_MS, now - k * WEEK_MS)))
        .collect();
    VisitRate {
        last_7_days: count(now - WEEK_MS, now),
        usual: median(&mut weeks),
    }
}

/// `true` when no silence between the household's readings (and the window
/// edges) in `(now - span, now]` exceeds [`COVERAGE_MAX_SILENCE_MS`].
/// `household` is sorted.
pub fn covered(household: &[i64], now: i64, span_ms: i64) -> bool {
    let start = now - span_ms;
    let mut previous = start;
    for &at in household.iter().filter(|&&at| at > start && at <= now) {
        if at - previous > COVERAGE_MAX_SILENCE_MS {
            return false;
        }
        previous = at;
    }
    now - previous <= COVERAGE_MAX_SILENCE_MS
}

/// The most recent silence of at least [`GAP_MS`] between two household
/// readings at or before `now`, as `(last before, first after)`.
pub fn last_closed_gap(household: &[i64], now: i64) -> Option<(i64, i64)> {
    let upto: Vec<i64> = household.iter().copied().filter(|&at| at <= now).collect();
    upto.windows(2)
        .rev()
        .find(|pair| pair[1] - pair[0] >= GAP_MS)
        .map(|pair| (pair[0], pair[1]))
}

/// What a rule says about one pet (or the household) right now.
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// Tripped with its value (percent, ratio or hours) and the push text.
    Tripped {
        value: f64,
        title: String,
        message: String,
    },
    /// Clearly back to normal; `recovery` is the follow-up push, if the rule
    /// sends one.
    Clear { recovery: Option<(String, String)> },
    /// Between thresholds or not enough data: keep the current state.
    Hold,
}

/// One rule's verdict for a pet (`pet_id` empty for household-wide rules).
#[derive(Clone, Debug, PartialEq)]
pub struct Assessment {
    pub pet_id: String,
    pub kind: PetHealthKind,
    pub signal: Signal,
}

/// The household key part for rules without a pet.
pub const HOUSEHOLD: &str = "*";

fn signed(percent: f64) -> String {
    if percent >= 0.0 {
        format!("+{percent:.1}%")
    } else {
        format!("{percent:.1}%")
    }
}

fn visit_line(visits: &VisitRate) -> String {
    match visits.usual {
        Some(usual) => format!(
            "Litter visits {}/wk (usual {}).",
            visits.last_7_days,
            format_count(usual)
        ),
        None => format!("Litter visits {}/wk.", visits.last_7_days),
    }
}

fn format_count(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Every rule for one pet. `household` is every pet's reading time, sorted.
pub fn assess_pet(
    pet_id: &str,
    name: &str,
    readings: &[Reading],
    household: &[i64],
    now: i64,
) -> Vec<Assessment> {
    let visits = visit_rate(readings, now);
    let two = weight_change(readings, now, 2 * WEEK_MS, MIN_RULE_READINGS);
    let four = weight_change(readings, now, 4 * WEEK_MS, MIN_RULE_READINGS);
    let ninety = weight_change(readings, now, 90 * DAY_MS, MIN_RULE_READINGS);
    let assessment = |kind, signal| Assessment {
        pet_id: pet_id.to_owned(),
        kind,
        signal,
    };

    let drop_2w = match (two, four) {
        (Some(two), Some(four))
            if two.drop() >= DROP_2W_PERCENT && four.drop() >= DROP_2W_CONFIRM_PERCENT =>
        {
            Signal::Tripped {
                value: two.drop(),
                title: format!("{name}: weight down {:.1}% in 2 weeks", two.drop()),
                message: format!(
                    "{name}: {:.2} lb this week, {} vs 2 weeks ago ({:.2} lb), {} vs 4 weeks ago. {}",
                    two.recent.median,
                    signed(two.percent),
                    two.baseline.median,
                    signed(four.percent),
                    visit_line(&visits)
                ),
            }
        }
        (Some(two), Some(four))
            if two.drop() < DROP_2W_CLEAR_PERCENT
                || four.drop() < DROP_2W_CONFIRM_CLEAR_PERCENT =>
        {
            Signal::Clear { recovery: None }
        }
        _ => Signal::Hold,
    };

    let drop_90d = match ninety {
        Some(change) if change.drop() >= DROP_90D_PERCENT => Signal::Tripped {
            value: change.drop(),
            title: format!("{name}: weight down {:.1}% in 90 days", change.drop()),
            message: format!(
                "{name}: {:.2} lb this week, {} vs 90 days ago ({:.2} lb). {}",
                change.recent.median,
                signed(change.percent),
                change.baseline.median,
                visit_line(&visits)
            ),
        },
        Some(change) if change.drop() < DROP_90D_CLEAR_PERCENT => Signal::Clear { recovery: None },
        _ => Signal::Hold,
    };

    let visit_drop = match visits.usual {
        Some(usual) if usual >= MIN_USUAL_VISITS && covered(household, now, WEEK_MS) => {
            let ratio = f64::from(visits.last_7_days) / usual;
            if ratio <= VISIT_DROP_RATIO {
                Signal::Tripped {
                    value: ratio,
                    title: format!("{name}: fewer litter-box visits"),
                    message: format!(
                        "{name}: {} litter-box visits in the last 7 days (usual {}/wk).",
                        visits.last_7_days,
                        format_count(usual)
                    ),
                }
            } else if ratio > VISIT_CLEAR_RATIO {
                Signal::Clear { recovery: None }
            } else {
                Signal::Hold
            }
        }
        _ => Signal::Hold,
    };

    vec![
        assessment(PetHealthKind::WeightDrop2w, drop_2w),
        assessment(PetHealthKind::WeightDrop90d, drop_90d),
        assessment(PetHealthKind::VisitDrop, visit_drop),
    ]
}

/// The household-wide gap rule. `household` is sorted; `tz` formats times.
pub fn assess_gap(household: &[i64], now: i64, tz: &jiff::tz::TimeZone) -> Assessment {
    let signal = match household.iter().copied().filter(|&at| at <= now).max() {
        None => Signal::Hold,
        Some(latest) if now - latest >= GAP_MS => {
            #[allow(clippy::cast_precision_loss)]
            let hours = (now - latest) as f64 / HOUR_MS as f64;
            Signal::Tripped {
                value: hours,
                title: "Pet scale: no readings for 48 h".to_owned(),
                message: format!(
                    "No litter-box readings for any pet since {} ({hours:.0} h). PetTracker keeps polling Whisker.",
                    local_time(latest, tz)
                ),
            }
        }
        Some(latest) => {
            let message = match last_closed_gap(household, now) {
                Some((before, after)) => {
                    #[allow(clippy::cast_precision_loss)]
                    let days = (after - before) as f64 / DAY_MS as f64;
                    format!(
                        "Litter-box readings resumed at {} after {days:.1} days without data.",
                        local_time(after, tz)
                    )
                }
                None => format!(
                    "Litter-box readings resumed (latest {}).",
                    local_time(latest, tz)
                ),
            };
            Signal::Clear {
                recovery: Some(("Pet scale: readings resumed".to_owned(), message)),
            }
        }
    };
    Assessment {
        pet_id: HOUSEHOLD.to_owned(),
        kind: PetHealthKind::DataGap,
        signal,
    }
}

/// `Oct 4, 13:11` in the configured zone.
pub fn local_time(ms: i64, tz: &jiff::tz::TimeZone) -> String {
    jiff::Timestamp::from_millisecond(ms).map_or_else(
        |_| to_iso_string(ms),
        |at| {
            at.to_zoned(tz.clone())
                .strftime("%b %-d, %H:%M")
                .to_string()
        },
    )
}

/// `weeks` rolling 7-day blocks ending at `now`, oldest first.
pub fn weekly_blocks(readings: &[Reading], now: i64, weeks: u32) -> Vec<PetWeek> {
    (0..i64::from(weeks))
        .rev()
        .map(|k| {
            let end = now - k * WEEK_MS;
            let start = end - WEEK_MS;
            PetWeek {
                start: to_iso_string(start),
                end: to_iso_string(end),
                readings: u32::try_from(window(readings, start, end).count()).unwrap_or(u32::MAX),
                median_weight: window_stat(readings, start, end).map(|s| round2(s.median)),
            }
        })
        .collect()
}

/// The displayed 2/4/12/26-week changes.
pub fn display_changes(readings: &[Reading], now: i64) -> Vec<PetWeightChange> {
    CHANGE_WEEKS
        .iter()
        .map(|&weeks| {
            let change = weight_change(
                readings,
                now,
                i64::from(weeks) * WEEK_MS,
                MIN_DISPLAY_READINGS,
            );
            PetWeightChange {
                weeks,
                percent: change.map(|c| round2(c.percent)),
                baseline_weight: change.map(|c| round2(c.baseline.median)),
            }
        })
        .collect()
}

/// The tripped assessments as findings.
pub fn findings(assessments: &[Assessment]) -> Vec<PetHealthFinding> {
    assessments
        .iter()
        .filter_map(|a| match &a.signal {
            Signal::Tripped { value, message, .. } => Some(PetHealthFinding {
                kind: a.kind,
                value: round2(*value),
                message: message.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Every pet's trend card and every rule's verdict at `now`.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub trends: Vec<PetTrend>,
    /// Pet rules, then the household gap rule last.
    pub assessments: Vec<Assessment>,
    pub latest_reading: Option<i64>,
}

impl Evaluation {
    /// The tripped household gap, if any.
    pub fn gap(&self) -> Option<PetHealthFinding> {
        findings(&self.assessments)
            .into_iter()
            .find(|f| f.kind == PetHealthKind::DataGap)
    }
}

/// Evaluates every pet (`weeks` weekly blocks per card).
pub fn evaluate(
    pets: &[PetWithHistory],
    now: i64,
    tz: &jiff::tz::TimeZone,
    weeks: u32,
) -> Evaluation {
    let parsed: Vec<Vec<Reading>> = pets.iter().map(|p| readings(&p.weight_history)).collect();
    let mut household: Vec<i64> = parsed.iter().flatten().map(|r| r.at_ms).collect();
    household.sort_unstable();
    let mut trends = Vec::with_capacity(pets.len());
    let mut assessments = Vec::new();
    for (pet, readings) in pets.iter().zip(&parsed) {
        let pet_assessments = assess_pet(&pet.pet.pet_id, &pet.pet.name, readings, &household, now);
        let visits = visit_rate(readings, now);
        trends.push(PetTrend {
            pet_id: pet.pet.pet_id.clone(),
            name: pet.pet.name.clone(),
            weight: window_stat(readings, now - WEEK_MS, now).map(|s| round2(s.median)),
            latest_reading_at: readings
                .iter()
                .rfind(|r| r.at_ms <= now)
                .map(|r| to_iso_string(r.at_ms)),
            weekly: weekly_blocks(readings, now, weeks),
            changes: display_changes(readings, now),
            visits_last_7_days: visits.last_7_days,
            usual_visits_per_week: visits.usual,
            findings: findings(&pet_assessments),
        });
        assessments.extend(pet_assessments);
    }
    assessments.push(assess_gap(&household, now, tz));
    Evaluation {
        trends,
        assessments,
        latest_reading: household.iter().copied().filter(|&at| at <= now).max(),
    }
}

/// The API and MCP response.
pub fn response(
    evaluation: &Evaluation,
    alerts: Vec<PetHealthAlertInfo>,
    now: i64,
) -> PetHealthResponse {
    PetHealthResponse {
        generated_at: to_iso_string(now),
        latest_reading_at: evaluation.latest_reading.map(to_iso_string),
        #[allow(clippy::cast_precision_loss)]
        hours_since_latest_reading: evaluation
            .latest_reading
            .map(|at| round2((now - at) as f64 / HOUR_MS as f64)),
        data_gap: evaluation.gap(),
        pets: evaluation.trends.clone(),
        alerts,
    }
}

/// `Sam 13.41 lb (-3.0%/2w), Sandy 12.45 lb (-2.6%/2w)` for the run summary.
pub fn summary_line(trends: &[PetTrend]) -> String {
    trends
        .iter()
        .map(|t| {
            let weight = t
                .weight
                .map_or_else(|| "no recent readings".to_owned(), |w| format!("{w:.2} lb"));
            let change = t
                .changes
                .iter()
                .find(|c| c.weeks == 2)
                .and_then(|c| c.percent)
                .map(|p| format!(" ({}/2w)", signed(p)))
                .unwrap_or_default();
            format!("{} {weight}{change}", t.name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The durable per-rule notification state the ledger keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleState {
    /// The rule is tripped (an episode is open).
    pub active: bool,
    /// This episode has been notified.
    pub notified: bool,
    pub episode_started_at: Option<i64>,
    pub last_notified_at: Option<i64>,
    pub last_value: Option<f64>,
    pub last_message: Option<String>,
    pub recovered_at: Option<i64>,
}

/// A push the state machine asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A new episode (at most one per pet and rule per [`WEEK_MS`]).
    Alert { title: String, message: String },
    /// The same episode, at least [`REMINDER_AFTER_MS`] later and at least
    /// [`REMINDER_WORSENING_PERCENT`] worse (weight rules only).
    Reminder { title: String, message: String },
    /// The episode ended (data gap only).
    Recovery { title: String, message: String },
}

impl Notice {
    pub fn title(&self) -> &str {
        match self {
            Notice::Alert { title, .. }
            | Notice::Reminder { title, .. }
            | Notice::Recovery { title, .. } => title,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Notice::Alert { message, .. }
            | Notice::Reminder { message, .. }
            | Notice::Recovery { message, .. } => message,
        }
    }
}

/// A still-open weight episode is repeated only after four weeks...
pub const REMINDER_AFTER_MS: i64 = 4 * WEEK_MS;
/// ...and only when it has worsened by this many percentage points.
pub const REMINDER_WORSENING_PERCENT: f64 = 2.0;

fn reminds(kind: PetHealthKind) -> bool {
    matches!(
        kind,
        PetHealthKind::WeightDrop2w | PetHealthKind::WeightDrop90d
    )
}

/// The next state and the push to send, if any. `None` state means no row.
pub fn decide(
    kind: PetHealthKind,
    previous: Option<&RuleState>,
    signal: &Signal,
    now: i64,
) -> (Option<RuleState>, Option<Notice>) {
    match signal {
        Signal::Hold => (previous.cloned(), None),
        Signal::Clear { recovery } => {
            let Some(prev) = previous.filter(|p| p.active) else {
                return (previous.cloned(), None);
            };
            let notice = recovery
                .as_ref()
                .filter(|_| prev.notified)
                .map(|(title, message)| Notice::Recovery {
                    title: title.clone(),
                    message: message.clone(),
                });
            let next = RuleState {
                active: false,
                notified: false,
                recovered_at: Some(now),
                ..prev.clone()
            };
            (Some(next), notice)
        }
        Signal::Tripped {
            value,
            title,
            message,
        } => {
            let mut next = match previous {
                Some(prev) if prev.active => prev.clone(),
                Some(prev) => RuleState {
                    active: true,
                    notified: false,
                    episode_started_at: Some(now),
                    ..prev.clone()
                },
                None => RuleState {
                    active: true,
                    notified: false,
                    episode_started_at: Some(now),
                    last_notified_at: None,
                    last_value: None,
                    last_message: None,
                    recovered_at: None,
                },
            };
            let since_last = next.last_notified_at.map(|at| now - at);
            let notice = if !next.notified {
                since_last
                    .is_none_or(|elapsed| elapsed >= WEEK_MS)
                    .then(|| Notice::Alert {
                        title: title.clone(),
                        message: message.clone(),
                    })
            } else if reminds(kind)
                && since_last.is_some_and(|elapsed| elapsed >= REMINDER_AFTER_MS)
                && next
                    .last_value
                    .is_some_and(|last| *value >= last + REMINDER_WORSENING_PERCENT)
            {
                Some(Notice::Reminder {
                    title: title.clone(),
                    message: message.clone(),
                })
            } else {
                None
            };
            if notice.is_some() {
                next.notified = true;
                next.last_notified_at = Some(now);
                next.last_value = Some(*value);
                next.last_message = Some(message.clone());
            }
            (Some(next), notice)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_000_000_000;

    /// `per_day` readings a day for `days` days ending at `NOW`, weight from
    /// `weight_at(days_ago)`.
    fn series(days: i64, per_day: i64, weight_at: impl Fn(f64) -> f64) -> Vec<Reading> {
        let step = DAY_MS / per_day;
        let mut out = Vec::new();
        let mut at = NOW - days * DAY_MS + step;
        while at <= NOW {
            #[allow(clippy::cast_precision_loss)]
            let days_ago = (NOW - at) as f64 / DAY_MS as f64;
            out.push(Reading {
                at_ms: at,
                weight: weight_at(days_ago),
            });
            at += step;
        }
        out
    }

    fn times(readings: &[Reading]) -> Vec<i64> {
        readings.iter().map(|r| r.at_ms).collect()
    }

    fn signal(assessments: &[Assessment], kind: PetHealthKind) -> Signal {
        assessments
            .iter()
            .find(|a| a.kind == kind)
            .map(|a| a.signal.clone())
            .unwrap_or(Signal::Hold)
    }

    /// Linear loss of `percent` per 14 days from 14 lb.
    fn declining(percent: f64) -> Vec<Reading> {
        series(120, 3, |days_ago| {
            14.0 * (1.0 + percent / 100.0 * days_ago / 14.0)
        })
    }

    #[test]
    fn parses_whisker_timestamps_as_utc() {
        assert_eq!(reading_ms("2026-10-09T04:27:58"), Some(1_791_520_078_000));
        assert_eq!(
            reading_ms("2026-10-09T04:27:58Z"),
            reading_ms("2026-10-09T04:27:58")
        );
        assert_eq!(reading_ms("nope"), None);
    }

    #[test]
    fn a_steady_three_and_a_half_percent_decline_trips_and_two_and_a_half_does_not() {
        let fast = declining(3.5);
        let a = assess_pet("p", "Sam", &fast, &times(&fast), NOW);
        let Signal::Tripped { value, message, .. } = signal(&a, PetHealthKind::WeightDrop2w) else {
            panic!("expected a trip: {a:?}");
        };
        assert!((3.0..4.0).contains(&value), "{value}");
        assert!(message.starts_with("Sam: "), "{message}");
        assert!(message.contains("vs 2 weeks ago"), "{message}");

        let slow = declining(2.5);
        let a = assess_pet("p", "Sam", &slow, &times(&slow), NOW);
        assert_eq!(signal(&a, PetHealthKind::WeightDrop2w), Signal::Hold);
    }

    #[test]
    fn a_dip_that_rebounds_within_four_weeks_does_not_trip() {
        // 13.0 four weeks ago, 13.5 two weeks ago, 13.05 now: -3.3% / 2w, +0.4% / 4w.
        let readings = series(40, 3, |days_ago| {
            if days_ago < 7.0 {
                13.05
            } else if days_ago < 21.0 {
                13.5
            } else {
                13.0
            }
        });
        let a = assess_pet("p", "Sandy", &readings, &times(&readings), NOW);
        assert_eq!(
            signal(&a, PetHealthKind::WeightDrop2w),
            Signal::Clear { recovery: None }
        );
    }

    #[test]
    fn fewer_than_eight_readings_yields_no_finding() {
        let sparse = series(120, 1, |days_ago| 14.0 * (1.0 + 0.05 * days_ago / 14.0));
        let sparse: Vec<Reading> = sparse
            .into_iter()
            .filter(|r| NOW - r.at_ms > WEEK_MS || (NOW - r.at_ms) < 5 * DAY_MS)
            .collect();
        let a = assess_pet("p", "Sam", &sparse, &times(&sparse), NOW);
        assert_eq!(signal(&a, PetHealthKind::WeightDrop2w), Signal::Hold);
        assert_eq!(signal(&a, PetHealthKind::WeightDrop90d), Signal::Hold);
    }

    #[test]
    fn readings_from_the_other_cat_are_excluded() {
        let mut readings = series(30, 3, |_| 13.4);
        // A burst of the other cat's (heavier) visits this week.
        for i in 0..6 {
            readings.push(Reading {
                at_ms: NOW - i * HOUR_MS,
                weight: 14.6,
            });
        }
        readings.sort_by_key(|r| r.at_ms);
        let stat = window_stat(&readings, NOW - WEEK_MS, NOW).unwrap();
        assert_eq!(stat.median, 13.4);
        assert_eq!(stat.count, 21);
    }

    #[test]
    fn ninety_day_drop_trips_at_five_percent() {
        let readings = series(110, 2, |days_ago| 13.0 * (1.0 + 0.06 * days_ago / 90.0));
        let a = assess_pet("p", "Sandy", &readings, &times(&readings), NOW);
        let Signal::Tripped { value, .. } = signal(&a, PetHealthKind::WeightDrop90d) else {
            panic!("{a:?}");
        };
        assert!(value >= 5.0, "{value}");
    }

    #[test]
    fn visit_drop_needs_coverage() {
        // 24 visits a week for ten weeks, then 7 this week.
        let mut readings = series(70, 3, |_| 13.0);
        readings.retain(|r| NOW - r.at_ms > WEEK_MS);
        for i in 0..7 {
            readings.push(Reading {
                at_ms: NOW - i * DAY_MS,
                weight: 13.0,
            });
        }
        readings.sort_by_key(|r| r.at_ms);
        let household = times(&readings);
        let a = assess_pet("p", "Sandy", &readings, &household, NOW);
        assert!(matches!(
            signal(&a, PetHealthKind::VisitDrop),
            Signal::Tripped { .. }
        ));
        // The same week inside a household outage is not judged.
        let outage: Vec<i64> = household
            .iter()
            .copied()
            .filter(|&at| NOW - at > 3 * DAY_MS || NOW - at < DAY_MS)
            .collect();
        let a = assess_pet("p", "Sandy", &readings, &outage, NOW);
        assert_eq!(signal(&a, PetHealthKind::VisitDrop), Signal::Hold);
    }

    #[test]
    fn a_seventy_two_hour_gap_trips_once_and_recovery_clears_it() {
        let tz = jiff::tz::TimeZone::UTC;
        let household = vec![NOW - 72 * HOUR_MS];
        let gap = assess_gap(&household, NOW, &tz);
        let (state, notice) = decide(PetHealthKind::DataGap, None, &gap.signal, NOW);
        assert!(matches!(notice, Some(Notice::Alert { .. })));
        let state = state.unwrap();
        // Still in the gap an hour later: nothing new.
        let later = assess_gap(&household, NOW + HOUR_MS, &tz);
        let (state, notice) = decide(
            PetHealthKind::DataGap,
            Some(&state),
            &later.signal,
            NOW + HOUR_MS,
        );
        assert_eq!(notice, None);
        // Readings resume.
        let resumed = vec![NOW - 72 * HOUR_MS, NOW + 2 * HOUR_MS];
        let back = assess_gap(&resumed, NOW + 2 * HOUR_MS, &tz);
        let (state, notice) = decide(
            PetHealthKind::DataGap,
            state.as_ref(),
            &back.signal,
            NOW + 2 * HOUR_MS,
        );
        let Some(Notice::Recovery { message, .. }) = notice else {
            panic!("{notice:?}");
        };
        assert!(message.contains("after 3.1 days"), "{message}");
        let state = state.unwrap();
        assert!(!state.active && !state.notified);
        // A second recovery evaluation sends nothing.
        let (_, notice) = decide(
            PetHealthKind::DataGap,
            Some(&state),
            &back.signal,
            NOW + 3 * HOUR_MS,
        );
        assert_eq!(notice, None);
    }

    fn tripped(value: f64) -> Signal {
        Signal::Tripped {
            value,
            title: "t".into(),
            message: format!("m {value}"),
        }
    }

    #[test]
    fn caps_alerts_at_one_per_week_per_rule() {
        let kind = PetHealthKind::WeightDrop2w;
        let (state, notice) = decide(kind, None, &tripped(3.1), NOW);
        assert!(notice.is_some());
        // The episode ends and a new one starts two days later: still capped.
        let (state, _) = decide(
            kind,
            state.as_ref(),
            &Signal::Clear { recovery: None },
            NOW + DAY_MS,
        );
        let (state, notice) = decide(kind, state.as_ref(), &tripped(3.2), NOW + 2 * DAY_MS);
        assert_eq!(notice, None);
        let state = state.unwrap();
        assert!(state.active && !state.notified);
        // Once the week has passed, the unnotified episode is reported.
        let (state, notice) = decide(kind, Some(&state), &tripped(3.2), NOW + WEEK_MS);
        assert!(matches!(notice, Some(Notice::Alert { .. })));
        // Repeated ticks inside the episode stay quiet.
        let (state, notice) = decide(kind, state.as_ref(), &tripped(3.4), NOW + 2 * WEEK_MS);
        assert_eq!(notice, None);
        // Four weeks on and two points worse: one reminder.
        let (state, notice) = decide(kind, state.as_ref(), &tripped(5.3), NOW + 5 * WEEK_MS);
        assert!(matches!(notice, Some(Notice::Reminder { .. })));
        let (_, notice) = decide(
            kind,
            state.as_ref(),
            &tripped(5.3),
            NOW + 5 * WEEK_MS + DAY_MS,
        );
        assert_eq!(notice, None);
    }

    #[test]
    fn visit_episodes_never_remind() {
        let kind = PetHealthKind::VisitDrop;
        let (state, _) = decide(kind, None, &tripped(0.4), NOW);
        let (_, notice) = decide(kind, state.as_ref(), &tripped(0.1), NOW + 6 * WEEK_MS);
        assert_eq!(notice, None);
    }

    #[test]
    fn coverage_rejects_long_silences() {
        let household: Vec<i64> = (0..14).rev().map(|i| NOW - i * 12 * HOUR_MS).collect();
        assert!(covered(&household, NOW, WEEK_MS));
        let holey: Vec<i64> = household
            .into_iter()
            .filter(|&at| NOW - at < 2 * DAY_MS || NOW - at > 4 * DAY_MS)
            .collect();
        assert!(!covered(&holey, NOW, WEEK_MS));
        assert!(!covered(&[], NOW, WEEK_MS));
    }
}
