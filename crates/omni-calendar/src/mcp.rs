//! Calendar MCP tools over the primary iCloud calendar
//! ([`crate::primary`]) plus a bounded listing of email-created events.
//! Metadata and schemas live in [`defs`]; refinements JSON Schema cannot
//! express (real dates, IANA zones, cross-field rules) are checked here.

pub mod defs;

use std::collections::BTreeSet;

use jiff::civil::{Date, DateTime};
use jiff::{SignedDuration, Timestamp, ToSpan as _};
use omni_core::digest::sha256_hex;
use omni_core::js::locale_compare;
use omni_mcp_kit::{McpTool, ToolError, ToolMetaError, typed_tool};
use omni_store::Store;
use serde::Serialize;

use crate::persistence::{self, CreatedCalendarEvent};
use crate::primary::edit::{
    self, Alarm, NewEvent, Patch, Recurrence, RecurrenceEnd, TimedEnd, Timing, TimingPatch, Zone,
};
use crate::primary::expand;
use crate::primary::ics::IcsDoc;
use crate::primary::model::{self, EventView, SchedulingRole, Trigger};
use crate::primary::operations::{WriteAction, WriteOutcome, WriteRequest};
use crate::primary::rrule::{Freq, Until, WeekdayNum, parse_weekday, weekday_code};
use crate::primary::store::{ChangeKind, ChangeOrigin, ChangeRow, OperationRecord, StepKind};
use crate::primary::time::{self, EventTime, Zones, iso_date, iso_local, rfc3339};
use crate::primary::{FoundOccurrence, Freshness, PrimaryCalendar, PrimaryError, READ_MAX_AGE_MS};
use defs::*;

/// The longest list window.
const MAX_WINDOW_DAYS: i64 = 366;
/// Preview bodies are capped at this many bytes each.
const PREVIEW_BODY_MAX: usize = 64 * 1024;

fn tool_error(error: &PrimaryError) -> ToolError {
    match error {
        PrimaryError::Coded { code, .. } if *code == "invalid_input" => {
            ToolError::input(error.tool_text())
        }
        _ => ToolError::execute(error.tool_text()),
    }
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::input(format!("[invalid_input] {}", message.into()))
}

fn ms_text(ms: i64) -> String {
    Timestamp::from_millisecond(ms).map_or_else(|_| ms.to_string(), rfc3339)
}

/// A date or a local date-time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    Date(Date),
    Local(DateTime),
}

fn parse_point(field: &str, value: &str) -> Result<Point, ToolError> {
    if value.len() == 10 {
        return value
            .parse::<Date>()
            .map(Point::Date)
            .map_err(|_| invalid(format!("{field}: not a real date")));
    }
    value
        .parse::<DateTime>()
        .map(Point::Local)
        .map_err(|_| invalid(format!("{field}: not a real local date-time")))
}

fn zone(name: Option<&str>, default: &str) -> Result<Zone, ToolError> {
    let name = name.unwrap_or(default);
    Zone::parse(name).ok_or_else(|| invalid(format!("timeZone: unknown time zone {name:?}")))
}

fn alarm(input: &AlarmIn) -> Result<Alarm, ToolError> {
    match (input.minutes_before, input.at.as_deref()) {
        (Some(minutes), None) => Ok(Alarm::BeforeStart(minutes)),
        (None, Some(at)) => at
            .parse::<Timestamp>()
            .map(Alarm::At)
            .map_err(|_| invalid("alarms.at: must be a date-time with an offset")),
        _ => Err(invalid(
            "each alarm needs exactly one of minutesBefore or at",
        )),
    }
}

fn alarms(input: Option<&Vec<AlarmIn>>) -> Result<Option<Vec<Alarm>>, ToolError> {
    input
        .map(|list| list.iter().map(alarm).collect::<Result<Vec<_>, _>>())
        .transpose()
}

fn weekday_num(code: &str) -> Result<WeekdayNum, ToolError> {
    let upper = code.trim().to_ascii_uppercase();
    let split = upper.len().saturating_sub(2);
    let (ordinal, day) = upper.split_at(split);
    let weekday = parse_weekday(day)
        .ok_or_else(|| invalid(format!("byWeekday: unknown weekday {code:?}")))?;
    let ordinal = if ordinal.is_empty() {
        None
    } else {
        let n: i16 = ordinal
            .parse()
            .map_err(|_| invalid(format!("byWeekday: bad ordinal in {code:?}")))?;
        if n == 0 || n.abs() > 53 {
            return Err(invalid(format!("byWeekday: bad ordinal in {code:?}")));
        }
        Some(n)
    };
    Ok(WeekdayNum { ordinal, weekday })
}

fn small<T: TryFrom<i32>>(
    field: &str,
    values: &[i32],
    range: std::ops::RangeInclusive<i32>,
) -> Result<Vec<T>, ToolError> {
    values
        .iter()
        .map(|v| {
            if *v == 0 || !range.contains(v) {
                return Err(invalid(format!("{field}: {v} is out of range")));
            }
            T::try_from(*v).map_err(|_| invalid(format!("{field}: {v} is out of range")))
        })
        .collect()
}

fn recurrence(input: &RecurrenceIn) -> Result<Recurrence, ToolError> {
    let freq = match input.frequency {
        Frequency::Daily => Freq::Daily,
        Frequency::Weekly => Freq::Weekly,
        Frequency::Monthly => Freq::Monthly,
        Frequency::Yearly => Freq::Yearly,
    };
    let end = match (input.count, input.until_date.as_deref()) {
        (Some(_), Some(_)) => return Err(invalid("recurrence: give count or untilDate, not both")),
        (Some(n), None) => RecurrenceEnd::Count(n),
        (None, Some(date)) => RecurrenceEnd::Until(
            date.parse::<Date>()
                .map_err(|_| invalid("recurrence.untilDate: not a real date"))?,
        ),
        (None, None) => RecurrenceEnd::Never,
    };
    Ok(Recurrence {
        freq,
        interval: input.interval.unwrap_or(1),
        by_day: input
            .by_weekday
            .iter()
            .flatten()
            .map(|c| weekday_num(c))
            .collect::<Result<_, _>>()?,
        by_month_day: small(
            "byMonthDay",
            input.by_month_day.as_deref().unwrap_or_default(),
            -31..=31,
        )?,
        by_month: small(
            "byMonth",
            input.by_month.as_deref().unwrap_or_default(),
            1..=12,
        )?,
        by_set_pos: small(
            "bySetPos",
            input.by_set_pos.as_deref().unwrap_or_default(),
            -366..=366,
        )?,
        week_start: None,
        end,
    })
}

/// A full timing from create-style fields.
fn timing(
    start: &str,
    end: Option<&str>,
    duration_minutes: Option<u32>,
    time_zone: Option<&str>,
    default_tz: &str,
) -> Result<Timing, ToolError> {
    match parse_point("start", start)? {
        Point::Date(start) => {
            if duration_minutes.is_some() || time_zone.is_some() {
                return Err(invalid(
                    "an all-day event takes no durationMinutes or timeZone; use end for the last day",
                ));
            }
            let last = match end.map(|e| parse_point("end", e)).transpose()? {
                None => start,
                Some(Point::Date(last)) => last,
                Some(Point::Local(_)) => {
                    return Err(invalid("end: an all-day event ends on a date (YYYY-MM-DD)"));
                }
            };
            if last < start {
                return Err(invalid("end: the last day precedes the start"));
            }
            Ok(Timing::AllDay { start, last })
        }
        Point::Local(start) => {
            let zone = zone(time_zone, default_tz)?;
            let end = match (
                end.map(|e| parse_point("end", e)).transpose()?,
                duration_minutes,
            ) {
                (Some(_), Some(_)) => {
                    return Err(invalid("give end or durationMinutes, not both"));
                }
                (Some(Point::Local(end)), None) => TimedEnd::At(end, zone.clone()),
                (Some(Point::Date(_)), None) => {
                    return Err(invalid("end: a timed event ends at a local date-time"));
                }
                (None, Some(minutes)) => TimedEnd::Minutes(i64::from(minutes)),
                (None, None) => TimedEnd::Minutes(60),
            };
            Ok(Timing::Timed { start, zone, end })
        }
    }
}

fn scope(input: Option<&defs::Scope>) -> edit::Scope {
    match input {
        None | Some(defs::Scope::Series) => edit::Scope::Series,
        Some(defs::Scope::Occurrence) => edit::Scope::Occurrence,
        Some(defs::Scope::Following) => edit::Scope::Following,
    }
}

fn sends(input: Option<&AttendeeNotifications>) -> bool {
    matches!(input, Some(AttendeeNotifications::Send))
}

fn non_blank(field: &str, value: &str) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("{field}: must not be blank")));
    }
    Ok(trimmed.to_owned())
}

fn create_action(input: &CreateInput, default_tz: &str) -> Result<WriteAction, ToolError> {
    Ok(WriteAction::Create(NewEvent {
        timing: timing(
            &input.start,
            input.end.as_deref(),
            input.duration_minutes,
            input.time_zone.as_deref(),
            default_tz,
        )?,
        title: non_blank("title", &input.title)?,
        notes: input.notes.clone().filter(|n| !n.trim().is_empty()),
        location: input
            .location
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned),
        url: input.url.clone().filter(|u| !u.trim().is_empty()),
        free: input.free.unwrap_or(false),
        alarms: alarms(input.alarms.as_ref())?.unwrap_or_default(),
        recurrence: input.recurrence.as_ref().map(recurrence).transpose()?,
    }))
}

fn patch(changes: &Changes, default_tz: &str) -> Result<Patch, ToolError> {
    let timing_patch = match (&changes.start, &changes.move_start_to) {
        (Some(_), Some(_)) => return Err(invalid("give start or moveStartTo, not both")),
        (Some(start), None) => Some(TimingPatch::Full(timing(
            start,
            changes.end.as_deref(),
            changes.duration_minutes,
            changes.time_zone.as_deref(),
            default_tz,
        )?)),
        (None, Some(target)) => {
            if changes.end.is_some() || changes.duration_minutes.is_some() {
                return Err(invalid(
                    "moveStartTo keeps the length; send start to change it",
                ));
            }
            Some(match parse_point("moveStartTo", target)? {
                Point::Date(date) => {
                    if changes.time_zone.is_some() {
                        return Err(invalid("timeZone needs a time in moveStartTo"));
                    }
                    TimingPatch::MoveDate(date)
                }
                Point::Local(local) => TimingPatch::MoveTime {
                    local,
                    zone: changes
                        .time_zone
                        .as_deref()
                        .map(|z| zone(Some(z), default_tz))
                        .transpose()?,
                },
            })
        }
        (None, None) => {
            if changes.end.is_some()
                || changes.duration_minutes.is_some()
                || changes.time_zone.is_some()
            {
                return Err(invalid("end, durationMinutes and timeZone need start"));
            }
            None
        }
    };
    let text = |v: &Option<Option<String>>| {
        v.as_ref().map(|inner| {
            inner
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
    };
    Ok(Patch {
        title: changes
            .title
            .as_deref()
            .map(|t| non_blank("title", t))
            .transpose()?,
        notes: changes
            .notes
            .clone()
            .map(|n| n.filter(|s| !s.trim().is_empty())),
        location: text(&changes.location),
        url: text(&changes.url),
        free: changes.free,
        timing: timing_patch,
        alarms: alarms(changes.alarms.as_ref())?,
        recurrence: match &changes.recurrence {
            None => None,
            Some(None) => Some(None),
            Some(Some(r)) => Some(Some(recurrence(r)?)),
        },
    })
}

fn update_action(input: &UpdateInput, default_tz: &str) -> Result<WriteAction, ToolError> {
    let scope = scope(input.scope.as_ref());
    if scope != edit::Scope::Series && input.recurrence_id.is_none() {
        return Err(invalid(
            "recurrenceId is required for scope occurrence or following",
        ));
    }
    Ok(WriteAction::Update {
        event_id: input.event_id.clone(),
        expected_etag: input.etag.clone(),
        scope,
        recurrence_id: input.recurrence_id.clone(),
        patch: patch(&input.changes, default_tz)?,
        drop_exceptions: input.drop_exceptions.unwrap_or(false),
        send_notifications: sends(input.attendee_notifications.as_ref()),
    })
}

fn delete_action(input: &DeleteInput) -> Result<WriteAction, ToolError> {
    let scope = scope(input.scope.as_ref());
    if scope != edit::Scope::Series && input.recurrence_id.is_none() {
        return Err(invalid(
            "recurrenceId is required for scope occurrence or following",
        ));
    }
    Ok(WriteAction::Delete {
        event_id: input.event_id.clone(),
        expected_etag: input.etag.clone(),
        scope,
        recurrence_id: input.recurrence_id.clone(),
        send_notifications: sends(input.attendee_notifications.as_ref()),
    })
}

fn fingerprint<T: Serialize>(tool: &str, input: &T) -> String {
    let json = serde_json::to_string(input).unwrap_or_default();
    sha256_hex(format!("{tool}:{json}").as_bytes())
}

fn role_text(role: SchedulingRole) -> String {
    match role {
        SchedulingRole::None => "none",
        SchedulingRole::Organizer => "organizer",
        SchedulingRole::Attendee => "attendee",
    }
    .to_owned()
}

/// `(text, time zone)` of an event time.
fn time_text(t: &EventTime, zones: Option<&Zones<'_>>) -> (String, Option<String>) {
    match t {
        EventTime::Date(d) => (iso_date(*d), None),
        EventTime::Zoned { local, tzid } => (
            iso_local(*local),
            Some(
                zones
                    .and_then(|z| z.iana(tzid).map(|(name, _)| name))
                    .unwrap_or_else(|| tzid.clone()),
            ),
        ),
        EventTime::Utc(ts) => (
            iso_local(ts.to_zoned(jiff::tz::TimeZone::UTC).datetime()),
            Some("UTC".to_owned()),
        ),
        EventTime::Floating(local) => (iso_local(*local), None),
    }
}

fn occurrence_out(found: &FoundOccurrence) -> Occurrence {
    occurrence_view(&found.event_id, &found.occurrence)
}

pub(crate) fn occurrence_view(event_id: &str, o: &expand::Occurrence) -> defs::Occurrence {
    let (start, tz) = time_text(&o.start, None);
    let (end, _) = time_text(&o.end, None);
    let tz = match &o.start {
        EventTime::Zoned { tzid, .. } => {
            time::resolve_tzid(tzid, None).map(|(name, _)| name).or(tz)
        }
        _ => tz,
    };
    defs::Occurrence {
        event_id: event_id.to_owned(),
        recurrence_id: o.recurrence_id.clone(),
        title: o.title.clone(),
        start,
        end,
        start_utc: rfc3339(o.start_utc),
        end_utc: rfc3339(o.end_utc),
        all_day: o.all_day,
        last_date: o.last_date.map(iso_date),
        time_zone: tz,
        location: o.location.clone(),
        recurring: o.recurring,
        is_exception: o.is_override,
        has_alarms: o.has_alarms,
        scheduling_role: role_text(o.scheduling_role),
        free: o.free,
        status: o.status.clone(),
    }
}

/// `[from, to)` from optional window points in the default zone.
pub(crate) fn window(
    service: &PrimaryCalendar,
    from: Option<&str>,
    to: Option<&str>,
    default_back_days: i64,
    default_span_days: i64,
) -> Result<(Timestamp, Timestamp), ToolError> {
    let tz = service.default_tz().clone();
    let instant = |field: &str, value: &str| -> Result<Timestamp, ToolError> {
        let local = match parse_point(field, value)? {
            Point::Date(d) => d.to_datetime(jiff::civil::Time::midnight()),
            Point::Local(dt) => dt,
        };
        time::compatible(&tz, local).ok_or_else(|| invalid(format!("{field}: out of range")))
    };
    let today = service.now().to_zoned(tz.clone()).date();
    let from = match from {
        Some(f) => instant("from", f)?,
        None => {
            let day = today
                .checked_sub(default_back_days.days())
                .map_err(|e| invalid(e.to_string()))?;
            time::compatible(&tz, day.to_datetime(jiff::civil::Time::midnight()))
                .ok_or_else(|| invalid("from: out of range"))?
        }
    };
    let to = match to {
        Some(t) => instant("to", t)?,
        None => from
            .checked_add(SignedDuration::from_hours(24 * default_span_days))
            .map_err(|e| invalid(e.to_string()))?,
    };
    if to <= from {
        return Err(invalid("to must be after from"));
    }
    if to.duration_since(from) > SignedDuration::from_hours(24 * MAX_WINDOW_DAYS) {
        return Err(invalid(format!(
            "the window may span at most {MAX_WINDOW_DAYS} days"
        )));
    }
    Ok((from, to))
}

async fn fresh(service: &PrimaryCalendar, force: bool) -> Result<Freshness, ToolError> {
    service
        .ensure_fresh(if force { 0 } else { READ_MAX_AGE_MS })
        .await
        .map_err(|e| tool_error(&e))
}

fn recurrence_out(rule_text: &str) -> RecurrenceOut {
    match crate::primary::rrule::RRule::parse(rule_text) {
        Ok(rule) => RecurrenceOut {
            rule: rule_text.to_owned(),
            editable: rule.is_editable(),
            frequency: match rule.freq {
                Freq::Daily => Some(Frequency::Daily),
                Freq::Weekly => Some(Frequency::Weekly),
                Freq::Monthly => Some(Frequency::Monthly),
                Freq::Yearly => Some(Frequency::Yearly),
                _ => None,
            },
            interval: rule.interval,
            by_weekday: rule
                .by_day
                .iter()
                .map(|w| {
                    format!(
                        "{}{}",
                        w.ordinal.map(|o| o.to_string()).unwrap_or_default(),
                        weekday_code(w.weekday)
                    )
                })
                .collect(),
            by_month_day: rule.by_month_day.iter().map(|v| i32::from(*v)).collect(),
            by_month: rule.by_month.iter().map(|v| i32::from(*v)).collect(),
            by_set_pos: rule.by_set_pos.iter().map(|v| i32::from(*v)).collect(),
            count: rule.count,
            until_date: match &rule.until {
                Some(Until::Date(d)) => Some(iso_date(*d)),
                Some(Until::Utc(ts)) => Some(iso_date(ts.to_zoned(jiff::tz::TimeZone::UTC).date())),
                Some(Until::Local(dt)) => Some(iso_date(dt.date())),
                None => None,
            },
        },
        Err(_) => RecurrenceOut {
            rule: rule_text.to_owned(),
            editable: false,
            frequency: None,
            interval: 1,
            by_weekday: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            count: None,
            until_date: None,
        },
    }
}

fn attendee(p: &model::Participant) -> Attendee {
    Attendee {
        email: p.email.clone(),
        name: p.name.clone(),
        status: p.partstat.clone(),
    }
}

/// The detail view of one resource.
pub fn event_detail(
    event_id: &str,
    etag: &str,
    text: &str,
    service: &PrimaryCalendar,
    freshness: &Freshness,
) -> Result<EventDetail, ToolError> {
    let doc = IcsDoc::parse(text).map_err(|e| {
        ToolError::execute(format!(
            "[unparseable_event] the event cannot be read: {}",
            e.0
        ))
    })?;
    let zones = Zones::new(&doc, service.default_tz().clone());
    let owner = service.owner();
    let event = model::primary_event(&doc)
        .ok_or_else(|| ToolError::execute("[unparseable_event] the resource holds no event"))?;
    let start = event
        .start()
        .ok_or_else(|| ToolError::execute("[unparseable_event] the event has no start"))?;
    let end = event
        .span(&zones)
        .end_for(&start, event.dtend().as_ref(), &zones)
        .unwrap_or_else(|| start.clone());
    let (start_text, tz) = time_text(&start, Some(&zones));
    let (end_text, _) = time_text(&end, Some(&zones));
    let master = model::master(&doc);
    let master_start = master.as_ref().and_then(EventView::start);
    let exceptions = Exceptions {
        deleted: master
            .as_ref()
            .zip(master_start.as_ref())
            .map(|(m, ms)| {
                m.exdates()
                    .iter()
                    .filter_map(|ex| zones.key(ex, ms))
                    .collect()
            })
            .unwrap_or_default(),
        changed: model::overrides(&doc)
            .iter()
            .filter_map(|o| {
                let rid = o.recurrence_id()?;
                let key = match &master_start {
                    Some(ms) => zones.key(&rid, ms)?,
                    None => zones.key(&rid, &rid)?,
                };
                let (start, tz) = time_text(&o.start().unwrap_or(rid), Some(&zones));
                Some(ChangedOccurrence {
                    recurrence_id: key,
                    title: o.summary().unwrap_or_default(),
                    start,
                    time_zone: tz,
                })
            })
            .collect(),
    };
    let blockers = edit::write_blockers(&doc, &zones);
    let role = event.scheduling_role(&owner);
    let now = service.now();
    let upcoming = expand::expand(
        &doc,
        &zones,
        &owner,
        now,
        now.checked_add(SignedDuration::from_hours(24 * MAX_WINDOW_DAYS))
            .unwrap_or(now),
    )
    .occurrences
    .iter()
    .take(5)
    .map(|o| occurrence_view(event_id, o))
    .collect();
    let mut blockers = blockers;
    if role == SchedulingRole::Attendee {
        blockers.push("invitation-read-only".to_owned());
    }
    Ok(EventDetail {
        event_id: event_id.to_owned(),
        etag: etag.to_owned(),
        title: event.summary().unwrap_or_default(),
        notes: event.description(),
        location: event.location(),
        url: event.url(),
        start: start_text,
        end: end_text,
        all_day: start.is_date(),
        last_date: match &start {
            EventTime::Date(d) => Some(iso_date(model::last_date(*d, Some(&end)))),
            _ => None,
        },
        time_zone: tz,
        free: event.is_free(),
        status: event.status(),
        recurrence: master
            .as_ref()
            .and_then(|m| m.rrules().first().cloned())
            .map(|r| recurrence_out(&r)),
        exceptions,
        alarms: event
            .alarms()
            .iter()
            .map(|a| AlarmOut {
                action: a.action.clone(),
                minutes_before: match a.trigger {
                    Some(Trigger::Start(m)) => Some(-m),
                    _ => None,
                },
                minutes_before_end: match a.trigger {
                    Some(Trigger::End(m)) => Some(-m),
                    _ => None,
                },
                at: match a.trigger {
                    Some(Trigger::At(ts)) => Some(rfc3339(ts)),
                    _ => None,
                },
            })
            .collect(),
        organizer: event.organizer().as_ref().map(attendee),
        attendees: event.attendees().iter().map(attendee).collect(),
        scheduling_role: role_text(role),
        writable: blockers.is_empty(),
        blockers,
        upcoming,
        synced_at: freshness.synced_at.map(ms_text),
        stale: freshness.stale,
    })
}

fn write_output(outcome: WriteOutcome) -> WriteOutput {
    WriteOutput {
        status: outcome.status,
        event_id: outcome.event_id,
        etag: outcome.etag,
        state: outcome.state,
        verified: outcome.verified,
        scheduling_notified: outcome.scheduling_notified,
        warnings: outcome.warnings,
        replayed: outcome.replayed,
    }
}

pub(crate) fn snapshot(p: &model::Projection) -> EventSnapshot {
    EventSnapshot {
        title: p.summary.clone(),
        start: p.start.clone(),
        start_utc: p.start_utc.clone(),
        all_day: p.all_day,
        recurring: p.recurring,
        time_zone: p.time_zone.clone(),
        location: p.location.clone(),
        status: p.status.clone(),
    }
}

pub fn change_out(row: &ChangeRow) -> Change {
    Change {
        cursor: row.seq.to_string(),
        event_id: row.event_id.clone(),
        uid: row.uid.clone(),
        kind: match row.kind {
            ChangeKind::Created => "created",
            ChangeKind::Updated => "updated",
            ChangeKind::Deleted => "deleted",
        }
        .to_owned(),
        origin: match row.origin {
            ChangeOrigin::Omni => "omni",
            ChangeOrigin::External => "external",
        }
        .to_owned(),
        changed_fields: row.changed_fields.clone(),
        version: match row.kind {
            ChangeKind::Deleted => None,
            _ => row.etag.clone(),
        },
        detected_at: ms_text(row.detected_at),
        before: row.before.as_ref().map(snapshot),
        after: row.after.as_ref().map(snapshot),
    }
}

fn lower<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn write_status_out(record: Option<OperationRecord>) -> WriteStatusOutput {
    let Some(record) = record else {
        return WriteStatusOutput {
            found: false,
            state: None,
            tool: None,
            created_at: None,
            updated_at: None,
            steps: Vec::new(),
            result: None,
            error: None,
        };
    };
    WriteStatusOutput {
        found: true,
        state: Some(lower(&record.state)),
        tool: Some(record.tool.clone()),
        created_at: Some(ms_text(record.created_at)),
        updated_at: Some(ms_text(record.updated_at)),
        steps: record
            .steps
            .iter()
            .map(|s| WriteStep {
                method: match s.kind {
                    StepKind::Put => "PUT",
                    StepKind::Delete => "DELETE",
                }
                .to_owned(),
                event_id: s.event_id.clone(),
                state: lower(&s.state),
                http_status: s.http_status,
                detail: s.detail.clone(),
            })
            .collect(),
        result: record
            .result
            .clone()
            .and_then(|v| serde_json::from_value::<WriteOutcome>(v).ok())
            .map(write_output),
        error: record.error.map(|e| defs::WriteError {
            code: e.code,
            message: e.message,
        }),
    }
}

fn tracked_view(e: &CreatedCalendarEvent) -> TrackedEvent {
    TrackedEvent {
        event_hash: e.event_hash.clone(),
        event_id: format!("{}.ics", e.calendar_event_id),
        source_email_id: e.email_id.clone(),
        title: e.title.clone(),
        start_date: e.start_date.clone(),
        start_time: e.start_time.clone(),
        end_date: e.end_date.clone(),
        end_time: e.end_time.clone(),
        all_day: e.all_day.unwrap_or(false),
        location: e.location.clone(),
        time_zone: e.time_zone.clone(),
        created_at: ms_text(e.created_at),
        status: if e.is_cancelled() {
            "cancelled"
        } else {
            "active"
        }
        .to_owned(),
    }
}

/// Shared state of the calendar tools.
#[derive(Clone)]
pub struct CalendarTools {
    pub store: Store,
    pub primary: PrimaryCalendar,
}

impl CalendarTools {
    async fn status(&self) -> Result<StatusOutput, ToolError> {
        let snapshot = self.primary.status(true).await;
        let events = persistence::get_tracked_events(&self.store)
            .await
            .map_err(|e| ToolError::execute_from(&e))?;
        let cancelled = events.iter().filter(|e| e.is_cancelled()).count() as u64;
        Ok(StatusOutput {
            configured: snapshot.configured,
            state: snapshot.state.to_owned(),
            message: snapshot.message,
            error_code: snapshot.error_code.map(str::to_owned),
            calendar_name: crate::primary::identity::PRIMARY_NAME.to_owned(),
            is_server_default: snapshot.is_server_default,
            pipeline_targets_primary: snapshot.pipeline_targets_primary,
            writable: snapshot.writable,
            supports_sync: snapshot.supports_sync,
            last_sync_at: snapshot.last_sync_at.map(ms_text),
            last_full_sync_at: snapshot.last_full_sync_at.map(ms_text),
            event_count: snapshot.event_count,
            change_cursor: snapshot.change_cursor.to_string(),
            default_time_zone: self.primary.default_tz_name().to_owned(),
            tracked: TrackedCounts {
                active: events.len() as u64 - cancelled,
                cancelled,
                total: events.len() as u64,
            },
        })
    }

    async fn list(&self, input: EventsListInput) -> Result<EventsListOutput, ToolError> {
        let (from, to) = window(
            &self.primary,
            input.from.as_deref(),
            input.to.as_deref(),
            0,
            14,
        )?;
        let freshness = fresh(&self.primary, input.fresh.unwrap_or(false)).await?;
        let (found, truncated) = self
            .primary
            .occurrences(from, to)
            .await
            .map_err(|e| tool_error(&e))?;
        let limit = input.limit.unwrap_or(100) as usize;
        let more = found.len() > limit;
        Ok(EventsListOutput {
            events: found.iter().take(limit).map(occurrence_out).collect(),
            from: rfc3339(from),
            to: rfc3339(to),
            truncated: truncated || more,
            synced_at: freshness.synced_at.map(ms_text),
            stale: freshness.stale,
            default_time_zone: self.primary.default_tz_name().to_owned(),
        })
    }

    async fn search(&self, input: EventsSearchInput) -> Result<EventsListOutput, ToolError> {
        let query = input.query.trim().to_lowercase();
        if query.is_empty() {
            return Err(invalid("query: must not be blank"));
        }
        let (from, to) = window(
            &self.primary,
            input.from.as_deref(),
            input.to.as_deref(),
            30,
            366,
        )?;
        let freshness = fresh(&self.primary, false).await?;
        let (found, truncated) = self
            .primary
            .occurrences(from, to)
            .await
            .map_err(|e| tool_error(&e))?;
        let limit = input.limit.unwrap_or(25) as usize;
        let mut seen = BTreeSet::new();
        let matches: Vec<&FoundOccurrence> = found
            .iter()
            .filter(|f| {
                let o = &f.occurrence;
                let haystack = format!(
                    "{}\n{}\n{}",
                    o.title,
                    o.location.as_deref().unwrap_or_default(),
                    o.notes.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                haystack.contains(&query)
            })
            .filter(|f| seen.insert((f.event_id.clone(), f.occurrence.title.clone())))
            .collect();
        Ok(EventsListOutput {
            events: matches
                .iter()
                .take(limit)
                .map(|f| occurrence_out(f))
                .collect(),
            from: rfc3339(from),
            to: rfc3339(to),
            truncated: truncated || matches.len() > limit,
            synced_at: freshness.synced_at.map(ms_text),
            stale: freshness.stale,
            default_time_zone: self.primary.default_tz_name().to_owned(),
        })
    }

    async fn get(&self, input: EventGetInput) -> Result<EventDetail, ToolError> {
        if !crate::primary::sync::is_valid_event_id(&input.event_id) {
            return Err(invalid(
                "eventId must be a resource name such as ABC-123.ics",
            ));
        }
        if input.fresh.unwrap_or(false) {
            let identity = self
                .primary
                .identity(false)
                .await
                .map_err(|e| tool_error(&e))?;
            let (etag, text) = self
                .primary
                .fetch(&identity, &input.event_id)
                .await
                .map_err(|e| tool_error(&e))?
                .ok_or_else(|| ToolError::execute("[not_found] no event with this eventId"))?;
            let freshness = Freshness {
                synced_at: Some(self.primary.now_ms()),
                ..Freshness::default()
            };
            return event_detail(&input.event_id, &etag, &text, &self.primary, &freshness);
        }
        let freshness = fresh(&self.primary, false).await?;
        let row = self
            .primary
            .mirror_row(&input.event_id)
            .await
            .map_err(|e| tool_error(&e))?
            .ok_or_else(|| ToolError::execute("[not_found] no event with this eventId"))?;
        let text = row.ics.ok_or_else(|| {
            ToolError::execute("[event_too_large] the event is too large to read here")
        })?;
        event_detail(&row.event_id, &row.etag, &text, &self.primary, &freshness)
    }

    fn action_of(&self, input: &PreviewInput) -> Result<(WriteAction, String), ToolError> {
        let tz = self.primary.default_tz_name();
        match (&input.create, &input.update, &input.delete) {
            (Some(c), None, None) => Ok((create_action(c, tz)?, c.idempotency_key.clone())),
            (None, Some(u), None) => Ok((update_action(u, tz)?, u.idempotency_key.clone())),
            (None, None, Some(d)) => Ok((delete_action(d)?, d.idempotency_key.clone())),
            _ => Err(invalid("give exactly one of create, update or delete")),
        }
    }

    async fn preview(&self, input: PreviewInput) -> Result<PreviewOutput, ToolError> {
        let (action, key) = self.action_of(&input)?;
        let prepared = self
            .primary
            .prepare(&action, &key)
            .await
            .map_err(|e| tool_error(&e))?;
        let plan = &prepared.plan;
        let mut changed = BTreeSet::new();
        for step in &plan.steps {
            let after = step.body.as_deref().and_then(|b| IcsDoc::parse(b).ok());
            let before = prepared
                .before
                .get(&step.event_id)
                .and_then(|b| IcsDoc::parse(b).ok());
            match (before, after) {
                (Some(b), Some(a)) => changed.extend(model::changed_fields(&b, &a)),
                (None, Some(_)) => {
                    changed.insert("created".to_owned());
                }
                (Some(_), None) => {
                    changed.insert("deleted".to_owned());
                }
                (None, None) => {}
            }
        }
        Ok(PreviewOutput {
            status: plan.status.as_str().to_owned(),
            writes: plan
                .steps
                .iter()
                .map(|s| PlannedWrite {
                    method: match s.kind {
                        StepKind::Put => "PUT",
                        StepKind::Delete => "DELETE",
                    }
                    .to_owned(),
                    event_id: s.event_id.clone(),
                    precondition: s.precondition.describe(),
                })
                .collect(),
            changed_fields: changed.into_iter().collect(),
            warnings: plan.warnings.clone(),
            scheduling_notified: plan.scheduling_notified,
            i_calendar: plan
                .steps
                .iter()
                .filter_map(|s| s.body.as_deref())
                .map(|b| {
                    if b.len() <= PREVIEW_BODY_MAX {
                        b.to_owned()
                    } else {
                        let mut end = PREVIEW_BODY_MAX;
                        while !b.is_char_boundary(end) {
                            end -= 1;
                        }
                        b[..end].to_owned()
                    }
                })
                .collect(),
        })
    }

    async fn write(
        &self,
        tool: &'static str,
        key: &str,
        fingerprint: String,
        action: WriteAction,
    ) -> Result<WriteOutput, ToolError> {
        self.primary
            .execute(WriteRequest {
                tool,
                idempotency_key: key.to_owned(),
                fingerprint,
                action,
            })
            .await
            .map(write_output)
            .map_err(|e| tool_error(&e))
    }

    async fn changes(&self, input: ChangesListInput) -> Result<ChangesListOutput, ToolError> {
        let cursor = input
            .cursor
            .as_deref()
            .map(|c| {
                c.parse::<i64>()
                    .map_err(|_| invalid("cursor: not a number"))
            })
            .transpose()?;
        let limit = input.limit.unwrap_or(50) as usize;
        let origin = match input.origin.unwrap_or_default() {
            ChangeOriginFilter::External => Some(ChangeOrigin::External),
            ChangeOriginFilter::Any => None,
        };
        let (rows, next, has_more) = self
            .primary
            .changes_since(cursor, limit, origin)
            .await
            .map_err(|e| tool_error(&e))?;
        Ok(ChangesListOutput {
            changes: rows.iter().map(change_out).collect(),
            next_cursor: next.to_string(),
            has_more,
        })
    }

    async fn tracked(&self, input: TrackedListInput) -> Result<TrackedListOutput, ToolError> {
        let query = input
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_lowercase);
        for (field, value) in [("from", &input.from), ("through", &input.through)] {
            if let Some(value) = value
                && value.parse::<Date>().is_err()
            {
                return Err(invalid(format!("{field}: not a real date")));
            }
        }
        let status = input.status.clone().unwrap_or_default();
        let mut events: Vec<CreatedCalendarEvent> = persistence::get_tracked_events(&self.store)
            .await
            .map_err(|e| ToolError::execute_from(&e))?
            .into_iter()
            .filter(|e| match status {
                TrackedStatusFilter::All => true,
                TrackedStatusFilter::Active => !e.is_cancelled(),
                TrackedStatusFilter::Cancelled => e.is_cancelled(),
            })
            .filter(|e| {
                input
                    .from
                    .as_deref()
                    .is_none_or(|f| e.start_date.as_str() >= f)
            })
            .filter(|e| {
                input
                    .through
                    .as_deref()
                    .is_none_or(|t| e.start_date.as_str() <= t)
            })
            .filter(|e| {
                query.as_deref().is_none_or(|q| {
                    format!(
                        "{}\n{}\n{}",
                        e.title,
                        e.location.as_deref().unwrap_or_default(),
                        e.description.as_deref().unwrap_or_default()
                    )
                    .to_lowercase()
                    .contains(q)
                })
            })
            .collect();
        let key = |e: &CreatedCalendarEvent| {
            format!(
                "{}T{}",
                e.start_date,
                e.start_time.as_deref().unwrap_or("00:00")
            )
        };
        events.sort_by(|a, b| locale_compare(&key(a), &key(b)));
        let cursor = input.cursor.unwrap_or(0) as usize;
        let limit = input.limit.unwrap_or(25) as usize;
        let total = events.len();
        let items = events
            .iter()
            .skip(cursor)
            .take(limit)
            .map(tracked_view)
            .collect();
        let next = cursor + limit;
        Ok(TrackedListOutput {
            items,
            next_cursor: (next < total).then(|| u32::try_from(next).unwrap_or(u32::MAX)),
            total: total as u64,
        })
    }
}

macro_rules! tool {
    ($t:expr, $def:expr, |$tools:ident, $input:ident : $ty:ty| $body:expr) => {{
        let tools = $t.clone();
        typed_tool(&$def, move |$input: $ty, _cx| {
            let $tools = tools.clone();
            async move { $body }
        })?
    }};
}

/// The calendar tools, in serving order.
pub fn calendar_tools(tools: CalendarTools) -> Result<Vec<McpTool>, ToolMetaError> {
    let t = tools;
    Ok(vec![
        tool!(t, CALENDAR_STATUS, |t, _input: StatusInput| t
            .status()
            .await),
        tool!(t, CALENDAR_EVENTS_LIST, |t, input: EventsListInput| t
            .list(input)
            .await),
        tool!(t, CALENDAR_EVENTS_SEARCH, |t, input: EventsSearchInput| t
            .search(input)
            .await),
        tool!(t, CALENDAR_EVENT_GET, |t, input: EventGetInput| t
            .get(input)
            .await),
        tool!(t, CALENDAR_EVENT_PREVIEW, |t, input: PreviewInput| t
            .preview(input)
            .await),
        tool!(t, CALENDAR_WRITE_STATUS, |t, input: WriteStatusInput| {
            t.primary
                .write_status(&input.idempotency_key)
                .await
                .map(write_status_out)
                .map_err(|e| tool_error(&e))
        }),
        tool!(t, CALENDAR_CHANGES_LIST, |t, input: ChangesListInput| t
            .changes(input)
            .await),
        tool!(
            t,
            CALENDAR_TRACKED_EVENTS_LIST,
            |t, input: TrackedListInput| t.tracked(input).await
        ),
        tool!(t, CALENDAR_EVENT_CREATE, |t, input: CreateInput| {
            let action = create_action(&input, t.primary.default_tz_name())?;
            let print = fingerprint("calendar_event_create", &input);
            t.write(
                "calendar_event_create",
                &input.idempotency_key,
                print,
                action,
            )
            .await
        }),
        tool!(t, CALENDAR_EVENT_UPDATE, |t, input: UpdateInput| {
            let action = update_action(&input, t.primary.default_tz_name())?;
            let print = fingerprint("calendar_event_update", &input);
            t.write(
                "calendar_event_update",
                &input.idempotency_key,
                print,
                action,
            )
            .await
        }),
        tool!(t, CALENDAR_EVENT_DELETE, |t, input: DeleteInput| {
            let action = delete_action(&input)?;
            let print = fingerprint("calendar_event_delete", &input);
            t.write(
                "calendar_event_delete",
                &input.idempotency_key,
                print,
                action,
            )
            .await
        }),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_weekday_codes() {
        assert_eq!(weekday_num("MO").unwrap().ordinal, None);
        assert_eq!(weekday_num("2tu").unwrap().ordinal, Some(2));
        assert_eq!(weekday_num("-1FR").unwrap().ordinal, Some(-1));
        assert!(weekday_num("0MO").is_err());
        assert!(weekday_num("XX").is_err());
    }

    #[test]
    fn timing_rules() {
        assert!(matches!(
            timing("2026-10-02", None, None, None, "America/Vancouver").unwrap(),
            Timing::AllDay { .. }
        ));
        assert!(timing("2026-10-02", None, Some(30), None, "America/Vancouver").is_err());
        assert!(
            timing(
                "2026-10-02T16:30",
                Some("2026-10-02T17:00"),
                Some(30),
                None,
                "UTC"
            )
            .is_err()
        );
        assert!(timing("2026-10-02T16:30", None, None, Some("Mars/Base"), "UTC").is_err());
        assert!(matches!(
            timing("2026-10-02T16:30", None, None, None, "America/Vancouver").unwrap(),
            Timing::Timed {
                end: TimedEnd::Minutes(60),
                ..
            }
        ));
    }
}
