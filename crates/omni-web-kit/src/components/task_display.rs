//! Run display helpers shared by Operations, Home and the log viewer.

use omni_api::runs::Run;

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
