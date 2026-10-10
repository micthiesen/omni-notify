//! Run display helpers shared by Operations, Home and the log viewer.

use omni_api::runs::{Run, RunStatus};

use super::tone::Tone;

use crate::utils::format::format_duration;
use crate::utils::js::now_ms;

/// Elapsed time of a run (until now while it is still running).
pub fn run_duration(run: &Run, now: f64) -> String {
    let end = run.finished_at.map_or(now, |f| f as f64);
    format_duration(end - run.started_at as f64)
}

/// [`run_duration`] against the current time.
pub fn run_duration_now(run: &Run) -> String {
    run_duration(run, now_ms())
}

/// A run's one-line detail and its tone: a failure's message (fault), a
/// degraded run's reason (warn, "Skipped: …"), a `skipped:` summary (warn) or
/// the plain summary.
pub fn run_detail(run: &Run) -> Option<(Tone, String)> {
    match run.status {
        RunStatus::Error => run
            .error
            .clone()
            .or_else(|| run.summary.clone())
            .map(|e| (Tone::Fault, e)),
        RunStatus::Degraded => {
            let reason = run.error.clone().or_else(|| run.summary.clone());
            Some((
                Tone::Warn,
                match reason {
                    Some(r) if r.to_lowercase().starts_with("skipped") => r,
                    Some(r) => format!("Skipped: {r}"),
                    None => "Skipped: an upstream step failed".to_owned(),
                },
            ))
        }
        RunStatus::Success | RunStatus::Running => run.summary.clone().map(|s| {
            let tone = if s.starts_with("skipped:") {
                Tone::Warn
            } else {
                Tone::Neutral
            };
            (tone, s)
        }),
    }
}

/// `row-sub` classes for a [`run_detail`] tone.
pub fn detail_class(tone: Tone) -> &'static str {
    match tone {
        Tone::Fault => "text-fault",
        Tone::Warn => "text-warn",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use omni_api::runs::RunTrigger;

    use super::*;

    fn run(status: RunStatus, error: Option<&str>, summary: Option<&str>) -> Run {
        Run {
            run_id: "r".into(),
            task_name: "T".into(),
            trigger: RunTrigger::Schedule,
            scheduled_for: None,
            started_at: 0,
            finished_at: Some(1),
            status,
            error: error.map(Into::into),
            summary: summary.map(Into::into),
        }
    }

    #[test]
    fn details_carry_the_reason_and_tone() {
        assert_eq!(
            run_detail(&run(RunStatus::Degraded, Some("Parcel 503"), None)),
            Some((Tone::Warn, "Skipped: Parcel 503".into()))
        );
        assert_eq!(
            run_detail(&run(RunStatus::Degraded, None, None)).map(|d| d.0),
            Some(Tone::Warn)
        );
        assert_eq!(
            run_detail(&run(RunStatus::Error, Some("boom"), Some("x"))),
            Some((Tone::Fault, "boom".into()))
        );
        assert_eq!(
            run_detail(&run(RunStatus::Success, None, Some("skipped: idle"))).map(|d| d.0),
            Some(Tone::Warn)
        );
        assert_eq!(run_detail(&run(RunStatus::Success, None, None)), None);
    }
}
