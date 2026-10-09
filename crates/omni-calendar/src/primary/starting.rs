//! `calendar.event_starting`: the `CalendarStartingEvents` scanner.
//!
//! Every 30 seconds, and only while a subscription is active, the scanner
//! refreshes the mirror (at most a minute old), expands it around now and
//! publishes each occurrence whose fire time has arrived, once per distinct
//! subscription tuple `(trigger, leadMinutes, includeAllDay)`. The dedup key
//! holds the occurrence's start instant, so an unchanged occurrence fires
//! once however often it is scanned and a rescheduled one fires again for its
//! new time.
//!
//! Timing rules:
//! - `start`: fires at `start - leadMinutes`. A fire time missed by more than
//!   [`ON_TIME_GRACE_MS`] (Omni was down, or the event was created inside the
//!   lead) still fires, marked `late`, while the occurrence has not started.
//! - `alarm`: fires at each VALARM's trigger instant, at most
//!   [`ALARM_LATE_LIMIT_MS`] late; an `ACKNOWLEDGED` at or after the fire
//!   time (dismissed on a device) suppresses it. `ACTION:NONE` alarms never
//!   fire, and an absolute trigger fires only for a single event or an
//!   override, where it names one instant.
//!
//! Cancelled occurrences never fire; deleted ones (EXDATE) are not expanded.

use std::collections::{BTreeMap, BTreeSet};

use futures::future::BoxFuture;
use jiff::{SignedDuration, Timestamp};
use omni_api::events::{
    CALENDAR_EVENT_STARTING, CalendarEventStarting, CalendarStartTrigger, bounded_title_flagged,
};
use omni_runtime::ports::{EventPublication, PortError};
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};
use serde_json::Value;

use super::events::port_error;
use super::model::Trigger;
use super::time::rfc3339;
use super::{FoundOccurrence, PrimaryCalendar, PrimaryError};

const LOG: &str = "CalendarPrimary";

pub const TASK_NAME: &str = "CalendarStartingEvents";
pub const SCHEDULE: &str = "*/30 * * * * *";
/// The scanner syncs first when the mirror is older than this.
pub const MIRROR_MAX_AGE_MS: i64 = 60_000;
/// A fire time at most this old counts as on time (scan interval plus a slow
/// sync).
pub const ON_TIME_GRACE_MS: i64 = 2 * 60_000;
/// Alarms later than this are history, not news.
pub const ALARM_LATE_LIMIT_MS: i64 = 10 * 60_000;
/// Alarm scans look this far ahead for occurrences whose alerts are due.
const ALARM_LOOKAHEAD: SignedDuration = SignedDuration::from_hours(8 * 24);
/// And this far back, for alerts relative to an occurrence's end.
const ALARM_LOOKBACK: SignedDuration = SignedDuration::from_hours(24);

/// One subscription tuple; the scanner publishes once per tuple.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tuple {
    Start {
        lead_minutes: i64,
        include_all_day: bool,
    },
    Alarm {
        include_all_day: bool,
    },
}

impl Tuple {
    fn include_all_day(&self) -> bool {
        match self {
            Tuple::Start {
                include_all_day, ..
            }
            | Tuple::Alarm { include_all_day } => *include_all_day,
        }
    }
}

/// The distinct tuples of canonical subscription arguments (invalid ones are
/// ignored; the catalog already rejected them at subscribe time).
pub fn tuples(arguments: &[BTreeMap<String, String>]) -> BTreeSet<Tuple> {
    arguments
        .iter()
        .filter_map(|args| {
            let include_all_day = match args.get("includeAllDay").map(String::as_str) {
                None | Some("false") => false,
                Some("true") => true,
                Some(_) => return None,
            };
            match args.get("trigger").map(String::as_str) {
                None | Some("start") => Some(Tuple::Start {
                    lead_minutes: args
                        .get("leadMinutes")
                        .map_or(Some(15), |lead| lead.parse().ok())?,
                    include_all_day,
                }),
                Some("alarm") => Some(Tuple::Alarm { include_all_day }),
                Some(_) => None,
            }
        })
        .collect()
}

/// The expansion window covering every tuple's due fire times.
pub fn window(tuples: &BTreeSet<Tuple>, now: Timestamp) -> (Timestamp, Timestamp) {
    let grace = SignedDuration::from_millis(ON_TIME_GRACE_MS);
    let mut from = now - grace;
    let mut to = now + SignedDuration::from_mins(2);
    for tuple in tuples {
        match tuple {
            Tuple::Start { lead_minutes, .. } => {
                to = to.max(now + SignedDuration::from_mins(lead_minutes + 2));
            }
            Tuple::Alarm { .. } => {
                from = from.min(now - ALARM_LOOKBACK);
                to = to.max(now + ALARM_LOOKAHEAD);
            }
        }
    }
    (from, to)
}

struct Firing {
    fire_at: Timestamp,
    late: bool,
    alarm_id: Option<String>,
}

fn ms(duration: SignedDuration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn start_firing(found: &FoundOccurrence, lead_minutes: i64, now: Timestamp) -> Option<Firing> {
    let start = found.occurrence.start_utc;
    let fire_at = start - SignedDuration::from_mins(lead_minutes);
    let lateness = ms(now.duration_since(fire_at));
    if lateness < 0 {
        return None;
    }
    let late = lateness > ON_TIME_GRACE_MS;
    // A late start still matters while the occurrence lies ahead.
    (!late || start > now).then_some(Firing {
        fire_at,
        late,
        alarm_id: None,
    })
}

fn alarm_firings(found: &FoundOccurrence, now: Timestamp) -> Vec<Firing> {
    let o = &found.occurrence;
    let single = o.recurrence_id.is_none() || o.is_override;
    o.alarms
        .iter()
        .enumerate()
        .filter(|(_, alarm)| alarm.action != "NONE")
        .filter_map(|(index, alarm)| {
            let fire_at = match alarm.trigger.as_ref()? {
                Trigger::Start(minutes) => o.start_utc + SignedDuration::from_mins(*minutes),
                Trigger::End(minutes) => o.end_utc + SignedDuration::from_mins(*minutes),
                Trigger::At(at) if single => *at,
                Trigger::At(_) => return None,
            };
            let lateness = ms(now.duration_since(fire_at));
            if !(0..=ALARM_LATE_LIMIT_MS).contains(&lateness) {
                return None;
            }
            if alarm.acknowledged.is_some_and(|at| at >= fire_at) {
                return None;
            }
            Some(Firing {
                fire_at,
                late: lateness > ON_TIME_GRACE_MS,
                alarm_id: Some(
                    alarm
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("{index}:{}", alarm.trigger_text)),
                ),
            })
        })
        .collect()
}

fn publication(found: &FoundOccurrence, tuple: &Tuple, firing: Firing) -> Option<EventPublication> {
    let o = &found.occurrence;
    let (summary, summary_truncated) = bounded_title_flagged(&o.title);
    let include_all_day = if tuple.include_all_day() {
        "true"
    } else {
        "false"
    };
    let (trigger, lead_minutes, discriminator) = match tuple {
        Tuple::Start { lead_minutes, .. } => (
            CalendarStartTrigger::Start,
            Some(lead_minutes.to_string()),
            format!("start:{lead_minutes}"),
        ),
        Tuple::Alarm { .. } => (
            CalendarStartTrigger::Alarm,
            None,
            format!("alarm:{}", firing.alarm_id.as_deref().unwrap_or_default()),
        ),
    };
    let start = rfc3339(o.start_utc);
    let uid = if o.uid.is_empty() {
        found.event_id.clone()
    } else {
        o.uid.clone()
    };
    let dedup_key = format!(
        "{uid}:{}:{discriminator}:{include_all_day}:{start}",
        o.recurrence_id.as_deref().unwrap_or("single"),
    );
    let payload = CalendarEventStarting {
        event_id: found.event_id.clone(),
        uid: o.uid.clone(),
        recurrence_id: o.recurrence_id.clone(),
        summary,
        summary_truncated,
        start,
        end: rfc3339(o.end_utc),
        all_day: o.all_day,
        time_zone: o.start.tzid().map(str::to_owned),
        trigger,
        lead_minutes,
        alarm_id: firing.alarm_id,
        fire_at: rfc3339(firing.fire_at),
        late: firing.late,
        has_location: o.location.as_deref().is_some_and(|l| !l.trim().is_empty()),
        include_all_day: include_all_day.to_owned(),
    };
    let Ok(Value::Object(data)) = serde_json::to_value(payload) else {
        return None;
    };
    Some(EventPublication {
        name: CALENDAR_EVENT_STARTING,
        dedup_key,
        occurred_at_ms: firing.fire_at.as_millisecond(),
        data,
    })
}

/// The publications due at `now` for these occurrences and tuples.
pub fn due_publications(
    found: &[FoundOccurrence],
    tuples: &BTreeSet<Tuple>,
    now: Timestamp,
) -> Vec<EventPublication> {
    let mut out = Vec::new();
    for occurrence in found {
        if occurrence.occurrence.status.as_deref() == Some("cancelled") {
            continue;
        }
        for tuple in tuples {
            if occurrence.occurrence.all_day && !tuple.include_all_day() {
                continue;
            }
            let firings = match tuple {
                Tuple::Start { lead_minutes, .. } => start_firing(occurrence, *lead_minutes, now)
                    .into_iter()
                    .collect(),
                Tuple::Alarm { .. } => alarm_firings(occurrence, now),
            };
            out.extend(
                firings
                    .into_iter()
                    .filter_map(|firing| publication(occurrence, tuple, firing)),
            );
        }
    }
    out
}

/// One scan; returns how many publications were handed to the port. Costs
/// one subscription lookup when nobody is subscribed.
pub async fn scan(service: &PrimaryCalendar) -> Result<usize, PrimaryError> {
    let Some(publisher) = service.ports().event_publisher() else {
        return Ok(0);
    };
    let arguments = publisher
        .active_arguments(CALENDAR_EVENT_STARTING)
        .await
        .map_err(port_error)?;
    let tuples = tuples(&arguments);
    if tuples.is_empty() {
        return Ok(0);
    }
    // A failed sync over an existing mirror scans the last good copy.
    service.ensure_fresh(MIRROR_MAX_AGE_MS).await?;
    let now = service.now();
    let (from, to) = window(&tuples, now);
    let (found, _) = service.occurrences(from, to).await?;
    let mut published = 0;
    for event in due_publications(&found, &tuples, now) {
        match publisher.publish(&event).await {
            Ok(_) => published += 1,
            Err(PortError::Failed {
                transient: false,
                message,
            }) => tracing::warn!(target: LOG, "Starting event rejected by MCP Events: {message}"),
            Err(error) => return Err(port_error(error)),
        }
    }
    Ok(published)
}

/// The `CalendarStartingEvents` task.
pub struct StartingEventsTask {
    service: PrimaryCalendar,
    schedule: CronSchedule,
}

impl StartingEventsTask {
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

impl Task for StartingEventsTask {
    fn name(&self) -> &str {
        TASK_NAME
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
            match scan(&self.service).await {
                Ok(_) => Ok(()),
                Err(error) if error.code() == "not_configured" => Ok(()),
                Err(error) => Err(TaskError::new(error.tool_text())),
            }
        })
    }
}
