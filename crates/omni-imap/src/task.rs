//! `EmailArchive` (`*/30 * * * * *`): the bounded archive sweep, run while the
//! transport is connected (a no-op before the first successful start).

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::archive_service::ArchiveService;
use crate::transport::ImapTransport;

pub const NAME: &str = "EmailArchive";
pub const SCHEDULE: &str = "*/30 * * * * *";

pub struct EmailArchiveTask {
    service: ArchiveService,
    transport: ImapTransport,
    schedule: CronSchedule,
}

impl EmailArchiveTask {
    pub fn new(service: ArchiveService, transport: ImapTransport, schedule: CronSchedule) -> Self {
        Self {
            service,
            transport,
            schedule,
        }
    }

    pub fn task(
        service: ArchiveService,
        transport: ImapTransport,
        tz: &jiff::tz::TimeZone,
    ) -> Result<Arc<dyn Task>, omni_tasks::InvalidScheduleError> {
        let schedule = CronSchedule::parse(SCHEDULE, tz)?;
        Ok(Arc::new(Self::new(service, transport, schedule)))
    }
}

impl Task for EmailArchiveTask {
    fn name(&self) -> &str {
        NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions {
            jitter: std::time::Duration::ZERO,
            run_on_startup: false,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            if !self.transport.is_active() {
                return Ok(());
            }
            self.service
                .sweep(&self.transport)
                .await
                .map_err(TaskError::from_error)
        })
    }
}
