//! The `RemindersSession` task: a 15-minute session check that never signs in.

use futures::future::BoxFuture;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::service::RemindersService;

pub const TASK_NAME: &str = "RemindersSession";
pub const SCHEDULE: &str = "*/15 * * * *";

/// Validates the saved Apple session; failures update the public status only.
pub struct RemindersSessionTask {
    service: RemindersService,
    schedule: CronSchedule,
}

impl RemindersSessionTask {
    pub fn new(
        service: RemindersService,
        tz: &jiff::tz::TimeZone,
    ) -> Result<Self, omni_tasks::InvalidScheduleError> {
        Ok(Self {
            service,
            schedule: CronSchedule::parse(SCHEDULE, tz)?,
        })
    }
}

impl Task for RemindersSessionTask {
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
            self.service.health_check().await;
            Ok(())
        })
    }
}
