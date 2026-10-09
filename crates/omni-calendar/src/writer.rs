//! The `CalendarWriter` port for workspace action approval: discover the
//! calendar, then create with the caller's deterministic UID
//! (`workspace-<actionId>@omni-notify`) so approval replays are idempotent.

use futures::future::BoxFuture;
use omni_runtime::ports::{
    CalendarCreateOutcome, CalendarEventInput, CalendarWriter, CalendarWriterStatus, PortError,
};

use crate::caldav::{Caldav, CreateOutcome};
use crate::extraction::schema::{EventAction, ExtractedEvent};

pub struct CaldavCalendarWriter {
    caldav: Caldav,
}

impl CaldavCalendarWriter {
    pub fn new(caldav: Caldav) -> Self {
        Self { caldav }
    }
}

fn to_event(input: &CalendarEventInput) -> ExtractedEvent {
    ExtractedEvent {
        action: EventAction::Create,
        event_id: None,
        title: input.title.clone(),
        start_date: input.start_date.clone(),
        end_date: input.end_date.clone(),
        start_time: input.start_time.clone(),
        end_time: input.end_time.clone(),
        duration: None,
        location: input.location.clone(),
        description: input.description.clone(),
        time_zone: input.time_zone.clone(),
        recurrence: None,
        all_day: input.all_day,
        reminder_minutes: input.reminder_minutes,
    }
}

impl CalendarWriter for CaldavCalendarWriter {
    fn create_event<'a>(
        &'a self,
        uid: &'a str,
        input: &'a CalendarEventInput,
    ) -> BoxFuture<'a, Result<CalendarCreateOutcome, PortError>> {
        Box::pin(async move {
            let failed =
                |message: String, transient: bool| PortError::Failed { message, transient };
            let session = self
                .caldav
                .discover()
                .await
                .map_err(|e| failed(e.to_string(), e.transient))?;
            match self
                .caldav
                .writer()
                .create(&session, &to_event(input), uid)
                .await
                .map_err(|e| failed(e.to_string(), e.transient))?
            {
                CreateOutcome::Success { event_uid } => {
                    Ok(CalendarCreateOutcome::Created { event_uid })
                }
                CreateOutcome::AlreadyExists { .. } => Ok(CalendarCreateOutcome::AlreadyExists),
                CreateOutcome::Error { code, message } => Err(failed(message, code >= 500)),
            }
        })
    }

    fn status(&self) -> CalendarWriterStatus {
        let provider = self.caldav.provider();
        CalendarWriterStatus {
            configured: provider.is_some(),
            provider: provider.map(str::to_owned),
        }
    }
}
