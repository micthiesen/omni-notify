//! Run health: degraded runs and the persistent-failure rule.
//!
//! A run is `degraded` when it completes without an error but skipped its real
//! work because an upstream failed (a Castro or Plex read, an unusable model
//! answer). Code inside a run reports that with [`report_degraded`]; the
//! registry collects the reasons per run through a task-local, so the report
//! works from any depth of the run's future and can never leak into another
//! run. "No work due" is never degraded.
//!
//! [`PersistenceRule`] decides when a streak of bad (failed or degraded) runs
//! is persistent enough to reach Pushover, and [`decide_incident`] turns
//! that into one notification per incident plus one recovery note.

use std::future::Future;
use std::sync::{Arc, Mutex};

use omni_store::cbor::Extra;
use omni_store::entity::Entity;
use serde::{Deserialize, Serialize};

use crate::persistence::{TaskRunData, TaskRunStatus};

/// Reasons kept per run; later distinct reasons are dropped.
pub const MAX_REASONS: usize = 5;
/// Each reason is cut to this many UTF-16 units.
pub const MAX_REASON_CHARS: usize = 300;

tokio::task_local! {
    static DEGRADED: Arc<Mutex<Vec<String>>>;
}

/// Marks the current run degraded with `reason`. Returns `false` outside a
/// collecting scope (for example code called from a route), where it is a
/// no-op. Duplicate reasons are kept once.
pub fn report_degraded(reason: impl Into<String>) -> bool {
    let reason = reason.into();
    DEGRADED
        .try_with(|slot| {
            let mut reasons = slot.lock().unwrap_or_else(|p| p.into_inner());
            let reason = bound_reason(&reason);
            if reasons.len() < MAX_REASONS && !reasons.contains(&reason) {
                reasons.push(reason);
            }
        })
        .is_ok()
}

fn bound_reason(reason: &str) -> String {
    let reason = reason.trim();
    if omni_core::js::utf16_len(reason) <= MAX_REASON_CHARS {
        reason.to_owned()
    } else {
        format!(
            "{}…",
            omni_core::js::utf16_slice(reason, 0, MAX_REASON_CHARS - 1)
        )
    }
}

/// Runs `fut` in a collecting scope and returns its output with the reasons
/// reported inside it, in report order.
pub async fn collect_degraded<F: Future>(fut: F) -> (F::Output, Vec<String>) {
    let slot = Arc::new(Mutex::new(Vec::new()));
    let output = DEGRADED.scope(slot.clone(), fut).await;
    let reasons = std::mem::take(&mut *slot.lock().unwrap_or_else(|p| p.into_inner()));
    (output, reasons)
}

/// The persisted reason text for a degraded run.
pub fn degraded_message(reasons: &[String]) -> Option<String> {
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

/// Whether a settled run counts against the task's health.
pub fn is_bad(status: TaskRunStatus) -> bool {
    matches!(status, TaskRunStatus::Error | TaskRunStatus::Degraded)
}

/// When a streak of bad runs is persistent: at least `min_runs` consecutive
/// bad runs whose start times span `min_span_ms` (less `jitter_ms` of
/// schedule jitter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersistenceRule {
    pub min_runs: usize,
    pub min_span_ms: i64,
    pub jitter_ms: i64,
}

/// Three consecutive bad runs spanning twelve hours, with five minutes of
/// cron jitter (the Castro inbox rule, generalized).
pub const PERSISTENT_FAILURE: PersistenceRule = PersistenceRule {
    min_runs: 3,
    min_span_ms: 12 * 60 * 60_000,
    jitter_ms: 5 * 60_000,
};

impl PersistenceRule {
    /// Whether the leading bad runs of `runs` (newest first) are persistent.
    /// A leading `running` row ends the streak.
    pub fn is_persistent(&self, runs: &[TaskRunData]) -> bool {
        let streak = bad_streak(runs);
        match (streak.first(), streak.last()) {
            (Some(newest), Some(oldest)) if streak.len() >= self.min_runs => {
                newest.started_at - oldest.started_at >= self.min_span_ms - self.jitter_ms
            }
            _ => false,
        }
    }
}

/// The leading run of bad statuses in newest-first `runs`.
pub fn bad_streak(runs: &[TaskRunData]) -> &[TaskRunData] {
    let len = runs.iter().take_while(|run| is_bad(run.status)).count();
    &runs[..len]
}

/// `task-health-incident`, keyed by `taskName`: a persistent bad streak that
/// has been notified. It exists only between the notification and the next
/// successful run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskHealthIncident {
    pub task_name: String,
    /// Start of the oldest bad run in the streak when it was notified.
    pub first_bad_at: i64,
    pub notified_at: i64,
    /// Bad runs in the streak when it was notified.
    pub bad_runs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for TaskHealthIncident {
    const NAME: &'static str = "task-health-incident";
    type Key = String;
    fn key(&self) -> String {
        self.task_name.clone()
    }
}

/// What to do after a run of a task settles.
#[derive(Clone, Debug, PartialEq)]
pub enum IncidentDecision {
    Nothing,
    /// Reserve the incident durably, then send its one notification.
    Notify(TaskHealthIncident),
    /// Delete the incident, then send its one recovery note.
    Recover(TaskHealthIncident),
}

/// Decides the incident transition from the task's newest-first history and
/// its stored incident. `running` rows are ignored (a next run may already
/// have started).
///
/// A persistent streak notifies only when it contains a degraded run: a
/// streak of plain errors already pages through the ERROR-log alert (and its
/// gates), so each path stays notified at exactly one layer. The incident
/// lasts until a successful run, even when the oldest bad runs are pruned
/// from history.
pub fn decide_incident(
    rule: &PersistenceRule,
    runs: &[TaskRunData],
    incident: Option<&TaskHealthIncident>,
    now: i64,
) -> IncidentDecision {
    let settled: Vec<TaskRunData> = runs
        .iter()
        .filter(|run| run.status != TaskRunStatus::Running)
        .cloned()
        .collect();
    if let Some(incident) = incident {
        let recovered = settled.iter().any(|run| {
            run.status == TaskRunStatus::Success && run.started_at > incident.first_bad_at
        });
        return if recovered {
            IncidentDecision::Recover(incident.clone())
        } else {
            IncidentDecision::Nothing
        };
    }
    let streak = bad_streak(&settled);
    if !rule.is_persistent(&settled)
        || !streak
            .iter()
            .any(|run| run.status == TaskRunStatus::Degraded)
    {
        return IncidentDecision::Nothing;
    }
    let (Some(newest), Some(oldest)) = (streak.first(), streak.last()) else {
        return IncidentDecision::Nothing;
    };
    IncidentDecision::Notify(TaskHealthIncident {
        task_name: newest.task_name.clone(),
        first_bad_at: oldest.started_at,
        notified_at: now,
        bad_runs: u32::try_from(streak.len()).unwrap_or(u32::MAX),
        reason: newest.error.clone(),
        extra: Extra::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collects_reports_from_nested_futures_and_dedups() {
        let ((), reasons) = collect_degraded(async {
            report_degraded("Castro GET failed");
            futures::join!(async { report_degraded("Castro GET failed") }, async {
                report_degraded("  Plex offline ")
            });
        })
        .await;
        assert_eq!(reasons, vec!["Castro GET failed", "Plex offline"]);
    }

    #[tokio::test]
    async fn reports_outside_a_scope_are_ignored() {
        assert!(!report_degraded("nobody listens"));
        let ((), reasons) = collect_degraded(async {}).await;
        assert!(reasons.is_empty());
    }

    #[tokio::test]
    async fn bounds_reason_count_and_length() {
        let ((), reasons) = collect_degraded(async {
            for i in 0..10 {
                report_degraded(format!("{i}{}", "x".repeat(400)));
            }
        })
        .await;
        assert_eq!(reasons.len(), MAX_REASONS);
        assert!(reasons.iter().all(|r| r.ends_with('…')));
        assert_eq!(omni_core::js::utf16_len(&reasons[0]), MAX_REASON_CHARS);
    }
}
