//! The `CalendarPrimarySync` task: keeps the mirror and change feed current,
//! every minute while a `calendar.*` MCP Events subscription is active and
//! every five minutes otherwise.

use futures::future::BoxFuture;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use omni_api::events::{CALENDAR_EVENT_CHANGED, CALENDAR_EVENT_STARTING};

use super::{BACKGROUND_MAX_AGE_MS, PrimaryCalendar, events};

pub const TASK_NAME: &str = "CalendarPrimarySync";
/// Every minute; network I/O only when the mirror is older than
/// [`BACKGROUND_MAX_AGE_MS`], or [`SUBSCRIBED_MAX_AGE_MS`] while subscribed.
pub const SCHEDULE: &str = "* * * * *";
/// Under the one-minute schedule, so a subscribed sync runs every tick.
pub const SUBSCRIBED_MAX_AGE_MS: i64 = 50_000;

/// The mirror age the task tolerates: short while any calendar event
/// subscription is active (a failed lookup counts as unsubscribed).
pub async fn max_age_ms(service: &PrimaryCalendar) -> i64 {
    for name in [CALENDAR_EVENT_CHANGED, CALENDAR_EVENT_STARTING] {
        if events::subscribed(service, name).await.unwrap_or(false) {
            return SUBSCRIBED_MAX_AGE_MS;
        }
    }
    BACKGROUND_MAX_AGE_MS
}

pub struct PrimarySyncTask {
    service: PrimaryCalendar,
    schedule: CronSchedule,
}

impl PrimarySyncTask {
    pub fn new(
        service: PrimaryCalendar,
        tz: &jiff::tz::TimeZone,
    ) -> Result<Self, omni_tasks::InvalidScheduleError> {
        Ok(Self {
            service,
            schedule: CronSchedule::parse(SCHEDULE, tz)?,
        })
    }
}

impl Task for PrimarySyncTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions::default()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            // Failures are recorded on the sync state and surface in
            // `calendar_status`; a stale mirror is served until recovery.
            let max_age = max_age_ms(&self.service).await;
            match self.service.ensure_fresh(max_age).await {
                Ok(freshness) if freshness.stale => Err(TaskError::new(
                    freshness
                        .error
                        .unwrap_or_else(|| "calendar sync failed".to_owned()),
                )),
                Ok(_) => Ok(()),
                Err(error) if error.code() == "not_configured" => Ok(()),
                Err(error) => Err(TaskError::new(error.tool_text())),
            }
        })
    }
}
