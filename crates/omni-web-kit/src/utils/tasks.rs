//! Task cadence, staleness and next-run formatting.

use omni_api::runs::{Run, RunStatus};
use omni_api::tasks::TaskInfo;

use super::js::parse_date_ms;

/// How often a task runs, from its next two fires.
pub fn period_ms(task: &TaskInfo) -> Option<f64> {
    let first = parse_date_ms(task.next_runs.first()?)?;
    let second = parse_date_ms(task.next_runs.get(1)?)?;
    (second > first).then_some(second - first)
}

/// Cadence group of the Operations table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cadence {
    /// Every minute or faster.
    Realtime,
    /// Under a day.
    Frequent,
    Scheduled,
}

impl Cadence {
    pub fn label(self) -> &'static str {
        match self {
            Cadence::Realtime => "Realtime",
            Cadence::Frequent => "Frequent",
            Cadence::Scheduled => "Scheduled",
        }
    }
}

pub fn cadence(task: &TaskInfo) -> Cadence {
    match period_ms(task) {
        Some(p) if p <= 60_000.0 => Cadence::Realtime,
        Some(p) if p < 86_400_000.0 => Cadence::Frequent,
        _ => Cadence::Scheduled,
    }
}

/// A task whose last run started more than three periods ago.
pub fn is_stale(task: &TaskInfo, now: f64) -> bool {
    match (period_ms(task), task.last_run.as_ref()) {
        (Some(period), Some(run)) => !task.running && now - run.started_at as f64 > 3.0 * period,
        _ => false,
    }
}

/// The task's health for status displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskHealth {
    Running,
    Fault,
    Stale,
    Ok,
    Idle,
}

pub fn task_health(task: &TaskInfo, now: f64) -> TaskHealth {
    if task.running {
        return TaskHealth::Running;
    }
    match &task.last_run {
        Some(Run {
            status: RunStatus::Error,
            ..
        }) => TaskHealth::Fault,
        Some(_) if is_stale(task, now) => TaskHealth::Stale,
        Some(_) => TaskHealth::Ok,
        None => TaskHealth::Idle,
    }
}

/// Next fire in epoch ms.
pub fn next_run_ms(task: &TaskInfo) -> Option<f64> {
    parse_date_ms(task.next_runs.first()?)
}

/// Compact time until a run: "0:06", "4m 12s", "4h 46m", "1d 20h", "due".
pub fn format_next(ms: f64) -> String {
    if ms <= 0.0 {
        return "due".to_owned();
    }
    let s = (ms / 1000.0).floor() as i64;
    let (d, h, m, sec) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60, s % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m >= 10 {
        format!("{m}m")
    } else {
        format!("{m}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(next: &[&str]) -> TaskInfo {
        TaskInfo {
            name: "T".into(),
            display_name: None,
            schedule: String::new(),
            running: false,
            next_runs: next.iter().map(|s| (*s).to_owned()).collect(),
            last_run: None,
        }
    }

    #[test]
    fn cadence_follows_the_fire_spacing() {
        let minutely = task(&["2026-01-01T00:00:00.000Z", "2026-01-01T00:01:00.000Z"]);
        let hourly = task(&["2026-01-01T00:00:00.000Z", "2026-01-01T01:00:00.000Z"]);
        let daily = task(&["2026-01-01T00:00:00.000Z", "2026-01-02T00:00:00.000Z"]);
        assert_eq!(cadence(&minutely), Cadence::Realtime);
        assert_eq!(cadence(&hourly), Cadence::Frequent);
        assert_eq!(cadence(&daily), Cadence::Scheduled);
        assert_eq!(cadence(&task(&[])), Cadence::Scheduled);
    }

    #[test]
    fn next_formats_by_magnitude() {
        assert_eq!(format_next(6_000.0), "0:06");
        assert_eq!(format_next(17_160_000.0), "4h 46m");
        assert_eq!(format_next(158_400_000.0), "1d 20h");
        assert_eq!(format_next(-1.0), "due");
    }
}
