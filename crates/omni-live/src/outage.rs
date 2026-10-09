//! Fleet-level unreachability alerts.
//!
//! One alert when an outage is confirmed, escalating reminders while it
//! lasts and one recovery note. The caller sends these directly instead of
//! through the generic alert throttle, which could swallow an escalation.

use omni_alerts::throttle::format_elapsed;

/// Consecutive all-unknown ticks before a streamer counts as unreachable.
pub const UNREACHABLE_TICK_THRESHOLD: u32 = 3;
/// Consecutive clean ticks before an outage is declared over.
pub const RECOVERY_TICK_THRESHOLD: u32 = 3;

const ESCALATION_MS: [i64; 4] = [
    30 * 60_000,
    2 * 60 * 60_000,
    6 * 60 * 60_000,
    24 * 60 * 60_000,
];
const MAX_NAMES: usize = 5;
const MAX_ERRORS: usize = 2;

/// One streamer whose bindings all returned unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownStreak {
    pub display_name: String,
    /// Consecutive all-unknown ticks so far.
    pub ticks: u32,
    /// Summary of the bindings' errors.
    pub error: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutageKind {
    Degraded,
    Recovered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutageAlert {
    pub kind: OutageKind,
    pub title: String,
    pub message: String,
}

#[derive(Clone, Debug)]
struct Episode {
    started_at: i64,
    last_alert_at: i64,
    alerts: usize,
    clear_ticks: u32,
}

/// Turns per-streamer unreachability into one outage-level alert stream.
#[derive(Clone, Debug, Default)]
pub struct OutageAlerter {
    episode: Option<Episode>,
}

impl OutageAlerter {
    /// Evaluates this tick; `streaks` lists every all-unknown streamer.
    pub fn evaluate(
        &mut self,
        streaks: &[UnknownStreak],
        total_streamers: usize,
        now: i64,
    ) -> Option<OutageAlert> {
        let confirmed: Vec<&UnknownStreak> = streaks
            .iter()
            .filter(|s| s.ticks >= UNREACHABLE_TICK_THRESHOLD)
            .collect();

        if confirmed.is_empty() {
            let episode = self.episode.as_mut()?;
            episode.clear_ticks += 1;
            if episode.clear_ticks < RECOVERY_TICK_THRESHOLD {
                return None;
            }
            let started_at = episode.started_at;
            self.episode = None;
            return Some(OutageAlert {
                kind: OutageKind::Recovered,
                title: "Live check recovered".to_owned(),
                message: format!(
                    "All streamers reachable again after {}.",
                    format_elapsed(now - started_at)
                ),
            });
        }

        let Some(episode) = self.episode.as_mut() else {
            self.episode = Some(Episode {
                started_at: now,
                last_alert_at: now,
                alerts: 1,
                clear_ticks: 0,
            });
            return Some(degraded_alert(&confirmed, total_streamers, 0));
        };

        episode.clear_ticks = 0;
        let wait = ESCALATION_MS[(episode.alerts - 1).min(ESCALATION_MS.len() - 1)];
        if now - episode.last_alert_at < wait {
            return None;
        }
        episode.last_alert_at = now;
        episode.alerts += 1;
        let elapsed = now - episode.started_at;
        Some(degraded_alert(&confirmed, total_streamers, elapsed))
    }
}

fn degraded_alert(confirmed: &[&UnknownStreak], total: usize, elapsed_ms: i64) -> OutageAlert {
    let mut sorted = confirmed.to_vec();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.ticks));
    let names: Vec<&str> = sorted
        .iter()
        .take(MAX_NAMES)
        .map(|s| s.display_name.as_str())
        .collect();
    let extra_names = sorted.len() - names.len();
    let mut lines = vec![if extra_names > 0 {
        format!("{} +{extra_names} more", names.join(", "))
    } else {
        names.join(", ")
    }];
    if elapsed_ms > 0 {
        lines.push(format!("Unreachable for {}.", format_elapsed(elapsed_ms)));
    }
    let mut distinct: Vec<&str> = Vec::new();
    for streak in &sorted {
        if !streak.error.is_empty() && !distinct.contains(&streak.error.as_str()) {
            distinct.push(&streak.error);
        }
    }
    let shown = distinct.len().min(MAX_ERRORS);
    if shown > 0 {
        lines.push(distinct[..shown].join("\n"));
    }
    if distinct.len() > shown {
        lines.push(format!("+{} other error(s)", distinct.len() - shown));
    }
    OutageAlert {
        kind: OutageKind::Degraded,
        title: format!(
            "Live check degraded: {}/{total} streamers unreachable",
            confirmed.len()
        ),
        message: lines.join("\n"),
    }
}
