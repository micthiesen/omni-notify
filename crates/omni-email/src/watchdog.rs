//! `EmailWatchdog`: warns when no email batch
//! has been dispatched for 72 hours (a June incident went 16 days unnoticed).

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_core::js::to_iso_string;
use omni_store::{Store, StoreError};
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::dispatch_state;

const LOG: &str = "Main:EmailWatchdog";
pub const NAME: &str = "EmailWatchdog";
/// Every 6 hours.
pub const SCHEDULE: &str = "0 0 */6 * * *";
pub const WATCHDOG_THRESHOLD_MS: i64 = 72 * 60 * 60_000;

/// Nothing dispatched within `threshold_ms` of `now`. Without
/// any dispatch, boot time stands in for the last dispatch.
pub fn should_warn(
    last_dispatched_at: Option<i64>,
    booted_at: i64,
    now: i64,
    threshold_ms: i64,
) -> bool {
    now - last_dispatched_at.unwrap_or(booted_at) > threshold_ms
}

pub struct EmailWatchdogTask {
    store: Store,
    schedule: CronSchedule,
    booted_at: i64,
    summary: Mutex<Option<String>>,
}

impl EmailWatchdogTask {
    /// `booted_at` is the process start (epoch ms).
    pub fn new(store: Store, schedule: CronSchedule, booted_at: i64) -> Self {
        Self {
            store,
            schedule,
            booted_at,
            summary: Mutex::new(None),
        }
    }

    fn set_summary(&self, summary: String) {
        *self.summary.lock().unwrap_or_else(|p| p.into_inner()) = Some(summary);
    }

    pub async fn check(&self) -> Result<(), StoreError> {
        let last = dispatch_state::last_dispatched_at(&self.store).await?;
        let now = self.store.clock().now_ms();
        if should_warn(last, self.booted_at, now, WATCHDOG_THRESHOLD_MS) {
            let since = match last {
                Some(at) => to_iso_string(at),
                None => format!("boot at {}", to_iso_string(self.booted_at)),
            };
            self.set_summary(format!("Stuck: no dispatch since {since}"));
            tracing::warn!(
                target: LOG,
                "No email has been dispatched since {since} — the email pipeline may be stuck"
            );
            omni_tasks::report_degraded(format!("no email dispatched since {since}"));
            return Ok(());
        }
        let Some(last) = last else {
            self.set_summary("No dispatch since boot yet (within threshold)".to_owned());
            tracing::info!(
                target: LOG,
                "No email dispatched since boot yet (still within watchdog threshold)"
            );
            return Ok(());
        };
        let at = to_iso_string(last);
        self.set_summary(format!("Healthy: last dispatch {at}"));
        tracing::info!(target: LOG, "Email pipeline healthy: last dispatch at {at}");
        Ok(())
    }
}

impl Task for EmailWatchdogTask {
    fn name(&self) -> &str {
        NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions::default()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move { self.check().await.map_err(TaskError::from_error) })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// Builds the task with its production schedule.
pub fn task(
    store: Store,
    tz: &jiff::tz::TimeZone,
    booted_at: i64,
) -> Result<Arc<dyn Task>, omni_tasks::InvalidScheduleError> {
    Ok(Arc::new(EmailWatchdogTask::new(
        store,
        CronSchedule::parse(SCHEDULE, tz)?,
        booted_at,
    )))
}

#[cfg(test)]
mod watchdog_task_spec {
    use super::*;

    const HOUR: i64 = 60 * 60_000;
    const NOW: i64 = 1_800_000_000_000;
    const BOOT: i64 = NOW - 100 * HOUR;

    #[test]
    fn does_not_warn_when_a_dispatch_happened_recently() {
        assert!(!should_warn(
            Some(NOW - HOUR),
            BOOT,
            NOW,
            WATCHDOG_THRESHOLD_MS
        ));
    }

    #[test]
    fn warns_when_the_last_dispatch_is_older_than_the_threshold() {
        assert!(should_warn(
            Some(NOW - 73 * HOUR),
            BOOT,
            NOW,
            WATCHDOG_THRESHOLD_MS
        ));
    }

    #[test]
    fn does_not_warn_exactly_at_the_threshold_boundary() {
        assert!(!should_warn(
            Some(NOW - WATCHDOG_THRESHOLD_MS),
            BOOT,
            NOW,
            WATCHDOG_THRESHOLD_MS
        ));
    }

    #[test]
    fn uses_boot_time_when_nothing_was_ever_dispatched() {
        assert!(!should_warn(
            None,
            NOW - 10 * HOUR,
            NOW,
            WATCHDOG_THRESHOLD_MS
        ));
        assert!(should_warn(
            None,
            NOW - 73 * HOUR,
            NOW,
            WATCHDOG_THRESHOLD_MS
        ));
    }

    #[test]
    fn respects_a_custom_threshold() {
        assert!(should_warn(Some(NOW - 2 * HOUR), BOOT, NOW, HOUR));
        assert!(!should_warn(Some(NOW - 2 * HOUR), BOOT, NOW, 3 * HOUR));
    }
}
