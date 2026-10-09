//! Calendar MCP tools: bounded reads over the
//! tracked events plus approval-gated CalDAV writes. Metadata and JSON schemas
//! are declared in [`defs`]; the refinements JSON Schema cannot express
//! (trim, real dates, IANA zones, cross-field rules) are checked here.

pub mod defs;

use omni_core::clock::SharedClock;
use omni_core::js::locale_compare;
use omni_mcp_kit::{McpTool, Page, ToolError, ToolMetaError, paginate, typed_tool};
use omni_store::Store;
use serde::{Deserialize, Serialize, Serializer};

use crate::caldav::{Caldav, CreateOutcome, DeleteOutcome, UpdateOutcome};
use crate::error::{CaldavError, CalendarPersistenceError};
use crate::extraction::sanitize::is_valid_time_zone;
use crate::extraction::schema::{EventAction, EventRecurrence, ExtractedEvent};
use crate::persistence::{
    self, CreatedCalendarEvent, EventFields, EventStatus, compute_event_hash,
    compute_mcp_event_uid, has_event_changed,
};

/// Serializes an `f64` the way `JSON.stringify` prints a JS number (no `.0`).
fn js_number<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if value.fract() == 0.0 && value.abs() <= MAX_SAFE {
        #[allow(clippy::cast_possible_truncation)]
        return s.serialize_i64(*value as i64);
    }
    s.serialize_f64(*value)
}

fn js_number_opt<S: Serializer>(value: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) => js_number(v, s),
        None => s.serialize_none(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedEventView {
    pub event_hash: String,
    pub calendar_event_id: String,
    pub source_email_id: String,
    pub title: String,
    pub start_date: String,
    pub start_time: Option<String>,
    pub end_date: Option<String>,
    pub end_time: Option<String>,
    pub all_day: bool,
    pub location: Option<String>,
    pub time_zone: Option<String>,
    pub description: Option<String>,
    pub duration: Option<String>,
    #[serde(serialize_with = "js_number_opt")]
    pub reminder_minutes: Option<f64>,
    pub recurrence: Option<EventRecurrence>,
    pub created_at: i64,
    pub status: &'static str,
}

impl From<&CreatedCalendarEvent> for TrackedEventView {
    fn from(e: &CreatedCalendarEvent) -> Self {
        Self {
            event_hash: e.event_hash.clone(),
            calendar_event_id: e.calendar_event_id.clone(),
            source_email_id: e.email_id.clone(),
            title: e.title.clone(),
            start_date: e.start_date.clone(),
            start_time: e.start_time.clone(),
            end_date: e.end_date.clone(),
            end_time: e.end_time.clone(),
            // Legacy rows without `allDay` read as timed events.
            all_day: e.all_day.unwrap_or(false),
            location: e.location.clone(),
            time_zone: e.time_zone.clone(),
            description: e.description.clone(),
            duration: e.duration.clone(),
            reminder_minutes: e.reminder_minutes,
            recurrence: e.recurrence.clone(),
            created_at: e.created_at,
            status: if e.is_cancelled() {
                "cancelled"
            } else {
                "active"
            },
        }
    }
}

/// Validated `calendarEventInputSchema` output (title and location trimmed).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventInput {
    pub title: String,
    pub start_date: String,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    pub all_day: bool,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub reminder_minutes: Option<f64>,
    #[serde(default)]
    pub recurrence: Option<EventRecurrence>,
}

/// A real calendar date in `YYYY-MM-DD` form.
pub fn is_valid_date(value: &str) -> bool {
    crate::extraction::sanitize::is_iso_date_shape(value)
        && value.parse::<jiff::civil::Date>().is_ok()
}

fn is_valid_time(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && (b[0] == b'0' || b[0] == b'1' || (b[0] == b'2' && b[1] <= b'3'))
        && b[3] <= b'5'
}

/// `^P(?=\d|T\d)(?:\d+D)?(?:T(?:\d+H)?(?:\d+M)?(?:\d+S)?)?$`.
fn is_valid_duration(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('P') else {
        return false;
    };
    let starts_ok = rest.starts_with(|c: char| c.is_ascii_digit())
        || rest
            .strip_prefix('T')
            .is_some_and(|t| t.starts_with(|c: char| c.is_ascii_digit()));
    if !starts_ok {
        return false;
    }
    let (date_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    let units = |part: &str, allowed: &[char]| -> bool {
        let mut digits = 0;
        let mut next_unit = 0;
        for c in part.chars() {
            if c.is_ascii_digit() {
                digits += 1;
            } else {
                let Some(pos) = allowed.iter().position(|u| *u == c) else {
                    return false;
                };
                if digits == 0 || pos < next_unit {
                    return false;
                }
                next_unit = pos + 1;
                digits = 0;
            }
        }
        digits == 0
    };
    units(date_part, &['D']) && time_part.is_none_or(|t| units(t, &['H', 'M', 'S']))
}

impl CalendarEventInput {
    /// Trims, then applies every refinement of `calendarEventInputSchema`.
    pub fn validated(mut self) -> Result<Self, ToolError> {
        let mut issues: Vec<String> = Vec::new();
        self.title = self.title.trim().to_owned();
        let title_len = omni_core::js::utf16_len(&self.title);
        if !(1..=200).contains(&title_len) {
            issues.push("title: must be 1-200 characters".to_owned());
        }
        if !is_valid_date(&self.start_date) {
            issues.push("startDate: Invalid date".to_owned());
        }
        if let Some(end_date) = &self.end_date
            && !is_valid_date(end_date)
        {
            issues.push("endDate: Invalid date".to_owned());
        }
        for (field, value) in [("startTime", &self.start_time), ("endTime", &self.end_time)] {
            if let Some(value) = value
                && !is_valid_time(value)
            {
                issues.push(format!("{field}: Expected a 24-hour time (HH:MM)"));
            }
        }
        if let Some(location) = self.location.take() {
            let location = location.trim().to_owned();
            let len = omni_core::js::utf16_len(&location);
            if !(1..=300).contains(&len) {
                issues.push("location: must be 1-300 characters".to_owned());
            }
            self.location = Some(location);
        }
        if let Some(tz) = &self.time_zone
            && (omni_core::js::utf16_len(tz) > 100 || !is_valid_time_zone(tz))
        {
            issues.push("timeZone: Expected a valid IANA time zone".to_owned());
        }
        if let Some(description) = &self.description
            && omni_core::js::utf16_len(description) > 2_000
        {
            issues.push("description: must be at most 2000 characters".to_owned());
        }
        if let Some(duration) = &self.duration
            && !is_valid_duration(duration)
        {
            issues.push("duration: Invalid ISO 8601 duration".to_owned());
        }
        if let Some(minutes) = self.reminder_minutes
            && (minutes.fract() != 0.0 || !(0.0..=40_320.0).contains(&minutes))
        {
            issues.push("reminderMinutes: must be an integer from 0 to 40320".to_owned());
        }
        if let Some(recurrence) = &self.recurrence
            && !is_valid_date(&recurrence.until)
        {
            issues.push("recurrence.until: Invalid date".to_owned());
        }
        if !self.all_day && self.start_time.is_none() {
            issues.push("startTime: Timed events require startTime".to_owned());
        }
        if self.all_day
            && (self.start_time.is_some() || self.end_time.is_some() || self.duration.is_some())
        {
            issues.push("allDay: All-day events cannot include times or duration".to_owned());
        }
        if self.end_time.is_some() && self.start_time.is_none() {
            issues.push("endTime: endTime requires startTime".to_owned());
        }
        if self.end_time.is_some() && self.duration.is_some() {
            issues.push("duration: Use either endTime or duration, not both".to_owned());
        }
        if let Some(end_date) = &self.end_date
            && end_date.as_str() < self.start_date.as_str()
        {
            issues.push("endDate: endDate cannot precede startDate".to_owned());
        }
        if let Some(recurrence) = &self.recurrence
            && recurrence.until.as_str() < self.start_date.as_str()
        {
            issues.push("recurrence.until: recurrence.until cannot precede startDate".to_owned());
        }
        if issues.is_empty() {
            Ok(self)
        } else {
            Err(ToolError::input(issues.join("; ")))
        }
    }

    fn to_event(&self, action: EventAction) -> ExtractedEvent {
        ExtractedEvent {
            action,
            event_id: None,
            title: self.title.clone(),
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
            start_time: self.start_time.clone(),
            end_time: self.end_time.clone(),
            duration: self.duration.clone(),
            location: self.location.clone(),
            description: self.description.clone(),
            time_zone: self.time_zone.clone(),
            recurrence: self.recurrence.clone(),
            all_day: self.all_day,
            reminder_minutes: self.reminder_minutes,
        }
    }

    fn from_record(record: &CreatedCalendarEvent) -> Self {
        Self {
            title: record.title.clone(),
            start_date: record.start_date.clone(),
            start_time: record.start_time.clone(),
            end_date: record.end_date.clone(),
            end_time: record.end_time.clone(),
            all_day: record.all_day.unwrap_or(false),
            location: record.location.clone(),
            time_zone: record.time_zone.clone(),
            description: record.description.clone(),
            duration: record.duration.clone(),
            reminder_minutes: record.reminder_minutes,
            recurrence: record.recurrence.clone(),
        }
    }
}

/// Absent keeps a field, `null` clears it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub start_time: Option<Option<String>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub end_date: Option<Option<String>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub end_time: Option<Option<String>>,
    #[serde(default)]
    pub all_day: Option<bool>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub location: Option<Option<String>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub time_zone: Option<Option<String>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub description: Option<Option<String>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub duration: Option<Option<String>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub reminder_minutes: Option<Option<f64>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub recurrence: Option<Option<EventRecurrence>>,
}

impl CalendarEventPatch {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn apply(self, mut base: CalendarEventInput) -> CalendarEventInput {
        fn set<T>(slot: &mut Option<T>, change: Option<Option<T>>) {
            if let Some(value) = change {
                *slot = value;
            }
        }
        if let Some(title) = self.title {
            base.title = title;
        }
        if let Some(start_date) = self.start_date {
            base.start_date = start_date;
        }
        if let Some(all_day) = self.all_day {
            base.all_day = all_day;
        }
        set(&mut base.start_time, self.start_time);
        set(&mut base.end_date, self.end_date);
        set(&mut base.end_time, self.end_time);
        set(&mut base.location, self.location);
        set(&mut base.time_zone, self.time_zone);
        set(&mut base.description, self.description);
        set(&mut base.duration, self.duration);
        set(&mut base.reminder_minutes, self.reminder_minutes);
        set(&mut base.recurrence, self.recurrence);
        base
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusFilter {
    #[default]
    Active,
    Cancelled,
    All,
}

fn default_limit() -> usize {
    25
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListInput {
    #[serde(default)]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    through: Option<String>,
    #[serde(default)]
    status: StatusFilter,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HashInput {
    event_hash: String,
}

#[derive(Debug, Deserialize)]
struct EventInputEnvelope {
    event: CalendarEventInput,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInput {
    event_hash: String,
    changes: CalendarEventPatch,
}

#[derive(Debug, Deserialize)]
struct EmptyInput {}

#[derive(Serialize)]
struct EventEnvelope {
    event: TrackedEventView,
}

#[derive(Serialize)]
struct StatusEnvelope {
    status: &'static str,
    event: TrackedEventView,
}

#[derive(Serialize)]
struct TrackedCounts {
    active: usize,
    cancelled: usize,
    total: usize,
}

#[derive(Serialize)]
struct StatusOutput {
    configured: bool,
    provider: Option<&'static str>,
    tracked: TrackedCounts,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewOutput {
    event_hash: String,
    duplicate_tracked_event: bool,
    #[serde(rename = "iCalendar")]
    i_calendar: String,
}

fn persistence_error(error: &CalendarPersistenceError) -> ToolError {
    ToolError::execute_from(error)
}

/// Tool errors report the innermost cause, which for a CalDAV error is
/// its cause text.
fn caldav_error(error: &CaldavError) -> ToolError {
    ToolError::execute(error.cause.clone())
}

/// Shared state of the calendar tools.
#[derive(Clone)]
pub struct CalendarTools {
    pub store: Store,
    pub caldav: Caldav,
    pub clock: SharedClock,
}

impl CalendarTools {
    async fn tracked_or_fail(&self, event_hash: &str) -> Result<CreatedCalendarEvent, ToolError> {
        persistence::get_tracked_event(&self.store, event_hash)
            .await
            .map_err(|e| persistence_error(&e))?
            .ok_or_else(|| {
                ToolError::execute(format!("Unknown tracked calendar event: {event_hash}"))
            })
    }

    async fn list(&self, input: ListInput) -> Result<Page<TrackedEventView>, ToolError> {
        // `query` is trimmed and must be 1 to 200 characters.
        let query = match input.query {
            Some(q) => {
                let trimmed = q.trim();
                if trimmed.is_empty() || omni_core::js::utf16_len(trimmed) > 200 {
                    return Err(ToolError::input("query: must be 1-200 characters"));
                }
                Some(trimmed.to_lowercase())
            }
            None => None,
        };
        for (field, value) in [("from", &input.from), ("through", &input.through)] {
            if let Some(value) = value
                && !is_valid_date(value)
            {
                return Err(ToolError::input(format!("{field}: Invalid date")));
            }
        }
        let mut events: Vec<CreatedCalendarEvent> = persistence::get_tracked_events(&self.store)
            .await
            .map_err(|e| persistence_error(&e))?
            .into_iter()
            .filter(|event| {
                let status = if event.is_cancelled() {
                    StatusFilter::Cancelled
                } else {
                    StatusFilter::Active
                };
                if input.status != StatusFilter::All && input.status != status {
                    return false;
                }
                if input
                    .from
                    .as_deref()
                    .is_some_and(|from| event.start_date.as_str() < from)
                {
                    return false;
                }
                if input
                    .through
                    .as_deref()
                    .is_some_and(|through| event.start_date.as_str() > through)
                {
                    return false;
                }
                if let Some(query) = &query {
                    let haystack = format!(
                        "{}\n{}\n{}",
                        event.title,
                        event.location.as_deref().unwrap_or_default(),
                        event.description.as_deref().unwrap_or_default()
                    )
                    .to_lowercase();
                    if !haystack.contains(query.as_str()) {
                        return false;
                    }
                }
                true
            })
            .collect();
        let sort_key = |e: &CreatedCalendarEvent| {
            format!(
                "{}T{}",
                e.start_date,
                e.start_time.as_deref().unwrap_or("00:00")
            )
        };
        events.sort_by(|a, b| locale_compare(&sort_key(a), &sort_key(b)));
        let views = events.iter().map(TrackedEventView::from).collect();
        Ok(paginate(views, input.cursor, input.limit))
    }

    async fn status(&self) -> Result<StatusOutput, ToolError> {
        let events = persistence::get_tracked_events(&self.store)
            .await
            .map_err(|e| persistence_error(&e))?;
        let cancelled = events.iter().filter(|e| e.is_cancelled()).count();
        let provider = self.caldav.provider();
        Ok(StatusOutput {
            configured: provider.is_some(),
            provider,
            tracked: TrackedCounts {
                active: events.len() - cancelled,
                cancelled,
                total: events.len(),
            },
        })
    }

    async fn preview(&self, input: CalendarEventInput) -> Result<PreviewOutput, ToolError> {
        let input = input.validated()?;
        let event = input.to_event(EventAction::Create);
        let event_hash =
            compute_event_hash(&event.title, &event.start_date, event.start_time.as_deref());
        let duplicate = persistence::has_created_event(&self.store, &event_hash)
            .await
            .map_err(|e| persistence_error(&e))?;
        Ok(PreviewOutput {
            i_calendar: self.caldav.writer().render(&event, "preview@omni-notify"),
            event_hash,
            duplicate_tracked_event: duplicate,
        })
    }

    async fn create(&self, input: CalendarEventInput) -> Result<StatusEnvelope, ToolError> {
        let input = input.validated()?;
        let event = input.to_event(EventAction::Create);
        let event_hash =
            compute_event_hash(&event.title, &event.start_date, event.start_time.as_deref());
        let existing = persistence::get_tracked_event(&self.store, &event_hash)
            .await
            .map_err(|e| persistence_error(&e))?;
        if let Some(existing) = existing.filter(|e| !e.is_cancelled()) {
            return Ok(StatusEnvelope {
                status: "already_exists",
                event: TrackedEventView::from(&existing),
            });
        }
        let session = self.caldav.discover().await.map_err(|e| caldav_error(&e))?;
        let uid = compute_mcp_event_uid(&event_hash);
        let (status, event_uid) = match self
            .caldav
            .writer()
            .create(&session, &event, &uid)
            .await
            .map_err(|e| caldav_error(&e))?
        {
            CreateOutcome::Success { event_uid } => ("created", event_uid),
            CreateOutcome::AlreadyExists { event_uid } => ("reconciled", event_uid),
            CreateOutcome::Error { message, .. } => return Err(ToolError::execute(message)),
        };
        let row = CreatedCalendarEvent::from_event(
            event_hash,
            "mcp".to_owned(),
            event_uid,
            &event,
            self.clock.now_ms(),
        );
        persistence::record_created_event(&self.store, row.clone())
            .await
            .map_err(|e| persistence_error(&e))?;
        Ok(StatusEnvelope {
            status,
            event: TrackedEventView::from(&row),
        })
    }

    async fn update(&self, input: UpdateInput) -> Result<StatusEnvelope, ToolError> {
        if input.changes.is_empty() {
            return Err(ToolError::input("At least one change is required"));
        }
        let existing = self.tracked_or_fail(&input.event_hash).await?;
        if existing.is_cancelled() {
            return Err(ToolError::execute("Cancelled events cannot be updated"));
        }
        // The merged event is re-validated during execution, so its failures are
        // execute-phase errors. A legacy record without `allDay` fails that check
        // unless the patch supplies it.
        let as_execute = |error: ToolError| ToolError::execute(error.message);
        if existing.all_day.is_none() && input.changes.all_day.is_none() {
            return Err(ToolError::execute("allDay: Required"));
        }
        let merged = input
            .changes
            .apply(CalendarEventInput::from_record(&existing))
            .validated()
            .map_err(as_execute)?;
        let event = merged.to_event(EventAction::Update);
        if !has_event_changed(&existing, &EventFields::from(&event)) {
            return Ok(StatusEnvelope {
                status: "unchanged",
                event: TrackedEventView::from(&existing),
            });
        }
        let new_hash =
            compute_event_hash(&event.title, &event.start_date, event.start_time.as_deref());
        if new_hash != existing.event_hash
            && let Some(prior) = persistence::get_tracked_event(&self.store, &new_hash)
                .await
                .map_err(|e| persistence_error(&e))?
                .filter(|p| !p.is_cancelled())
        {
            if prior.calendar_event_id != existing.calendar_event_id {
                return Err(ToolError::execute(
                    "Update would collide with another active tracked event",
                ));
            }
            // Reconcile a prior partial success: the replacement row was persisted
            // before the old row was tombstoned, so no remote write is needed.
            persistence::mark_event_cancelled(&self.store, &existing.event_hash)
                .await
                .map_err(|e| persistence_error(&e))?;
            return Ok(StatusEnvelope {
                status: "updated",
                event: TrackedEventView::from(&prior),
            });
        }
        let session = self.caldav.discover().await.map_err(|e| caldav_error(&e))?;
        match self
            .caldav
            .writer()
            .update(&session, &event, &existing.calendar_event_id)
            .await
            .map_err(|e| caldav_error(&e))?
        {
            UpdateOutcome::Success { .. } => {}
            UpdateOutcome::Error { message, .. } => return Err(ToolError::execute(message)),
        }
        let row = CreatedCalendarEvent::from_event(
            new_hash,
            existing.email_id.clone(),
            existing.calendar_event_id.clone(),
            &event,
            self.clock.now_ms(),
        );
        // Persist the replacement before tombstoning the old identity (one
        // transaction); a retry reconciles through the branch above.
        persistence::replace_created_event(&self.store, row.clone(), &existing.event_hash)
            .await
            .map_err(|e| persistence_error(&e))?;
        Ok(StatusEnvelope {
            status: "updated",
            event: TrackedEventView::from(&row),
        })
    }

    async fn delete(&self, event_hash: &str) -> Result<StatusEnvelope, ToolError> {
        let existing = self.tracked_or_fail(event_hash).await?;
        if existing.is_cancelled() {
            return Ok(StatusEnvelope {
                status: "already_deleted",
                event: TrackedEventView::from(&existing),
            });
        }
        let session = self.caldav.discover().await.map_err(|e| caldav_error(&e))?;
        match self
            .caldav
            .writer()
            .delete(&session, &existing.calendar_event_id)
            .await
            .map_err(|e| caldav_error(&e))?
        {
            DeleteOutcome::Success | DeleteOutcome::NotFound => {}
            DeleteOutcome::Error { message, .. } => return Err(ToolError::execute(message)),
        }
        persistence::mark_event_cancelled(&self.store, &existing.event_hash)
            .await
            .map_err(|e| persistence_error(&e))?;
        let mut cancelled = existing;
        cancelled.status = Some(EventStatus::Cancelled);
        Ok(StatusEnvelope {
            status: "deleted",
            event: TrackedEventView::from(&cancelled),
        })
    }
}

/// The seven calendar tools, in serving order.
pub fn calendar_tools(tools: CalendarTools) -> Result<Vec<McpTool>, ToolMetaError> {
    let t = tools;
    Ok(vec![
        {
            let t = t.clone();
            typed_tool(&defs::CALENDAR_EVENTS_LIST, move |input: ListInput, _cx| {
                let t = t.clone();
                async move { t.list(input).await }
            })?
        },
        {
            let t = t.clone();
            typed_tool(&defs::CALENDAR_EVENT_GET, move |input: HashInput, _cx| {
                let t = t.clone();
                async move {
                    Ok(EventEnvelope {
                        event: TrackedEventView::from(&t.tracked_or_fail(&input.event_hash).await?),
                    })
                }
            })?
        },
        {
            let t = t.clone();
            typed_tool(&defs::CALENDAR_STATUS, move |_input: EmptyInput, _cx| {
                let t = t.clone();
                async move { t.status().await }
            })?
        },
        {
            let t = t.clone();
            typed_tool(
                &defs::CALENDAR_EVENT_PREVIEW,
                move |input: EventInputEnvelope, _cx| {
                    let t = t.clone();
                    async move { t.preview(input.event).await }
                },
            )?
        },
        {
            let t = t.clone();
            typed_tool(
                &defs::CALENDAR_EVENT_CREATE,
                move |input: EventInputEnvelope, _cx| {
                    let t = t.clone();
                    async move { t.create(input.event).await }
                },
            )?
        },
        {
            let t = t.clone();
            typed_tool(
                &defs::CALENDAR_EVENT_UPDATE,
                move |input: UpdateInput, _cx| {
                    let t = t.clone();
                    async move { t.update(input).await }
                },
            )?
        },
        typed_tool(
            &defs::CALENDAR_EVENT_DELETE,
            move |input: HashInput, _cx| {
                let t = t.clone();
                async move { t.delete(&input.event_hash).await }
            },
        )?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_follow_the_golden_pattern() {
        for ok in [
            "PT1H", "P1D", "PT1H30M", "P1DT2H", "PT45S", "P2DT1M", "P1DT",
        ] {
            assert!(is_valid_duration(ok), "{ok}");
        }
        for bad in ["P", "PT", "1H", "PTH", "PT1M1H", "P1H", "PT1H2", "PD"] {
            assert!(!is_valid_duration(bad), "{bad}");
        }
    }

    #[test]
    fn times_and_dates() {
        assert!(is_valid_time("23:59"));
        assert!(!is_valid_time("24:00"));
        assert!(!is_valid_time("9:00"));
        assert!(is_valid_date("2028-02-29"));
        assert!(!is_valid_date("2026-02-30"));
    }

    #[test]
    fn js_numbers_have_no_trailing_fraction() {
        #[derive(Serialize)]
        struct N {
            #[serde(serialize_with = "js_number_opt")]
            n: Option<f64>,
        }
        assert_eq!(
            serde_json::to_string(&N { n: Some(60.0) }).ok().as_deref(),
            Some(r#"{"n":60}"#)
        );
        assert_eq!(
            serde_json::to_string(&N { n: Some(1.5) }).ok().as_deref(),
            Some(r#"{"n":1.5}"#)
        );
    }
}
