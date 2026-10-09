//! Extracted event shapes (`src/calendar-events/extraction/schema.ts`).
//!
//! One struct serves both as the strict structured-output schema the model fills
//! (optional values are nullable and required, OpenAI strict mode) and as the
//! normalized application shape (`null` and absent both become `None`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the model wants done with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EventAction {
    Create,
    Cancel,
    Update,
}

impl EventAction {
    pub fn as_str(self) -> &'static str {
        match self {
            EventAction::Create => "create",
            EventAction::Cancel => "cancel",
            EventAction::Update => "update",
        }
    }
}

/// RRULE frequency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
}

impl RecurrenceFrequency {
    pub fn as_str(self) -> &'static str {
        match self {
            RecurrenceFrequency::Daily => "daily",
            RecurrenceFrequency::Weekly => "weekly",
            RecurrenceFrequency::Monthly => "monthly",
        }
    }
}

/// Fixed repeat pattern for a recurring event (RRULE FREQ + inclusive UNTIL date).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct EventRecurrence {
    pub frequency: RecurrenceFrequency,
    #[schemars(description = "ISO 8601 date of the last occurrence (inclusive), e.g. 2026-07-13")]
    pub until: String,
}

/// One extracted event (`ExtractedCalendarEvent`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedEvent {
    #[schemars(
        description = "'create' for new events, 'cancel' for cancelled events, 'update' for rescheduled/modified events"
    )]
    pub action: EventAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "For 'cancel' and 'update' only: the id shown in square brackets next to the existing event this action refers to, WITHOUT the brackets (for [evt_2], use evt_2). Copy it exactly from the existing events list. Use null for 'create'"
    )]
    pub event_id: Option<String>,
    #[schemars(
        description = "Short event title prefixed with a relevant emoji in Title Case (e.g. '🦷 Dentist Appointment', '✈️ Flight to Vancouver')"
    )]
    pub title: String,
    #[schemars(description = "ISO 8601 date, e.g. 2026-03-20")]
    pub start_date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "ISO 8601 end date if different from startDate (e.g. multi-day hotel stay). Omit for single-day events"
    )]
    pub end_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "24-hour time, e.g. 14:30. Omit for all-day events")]
    pub start_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "24-hour end time, e.g. 16:00. Omit if unknown")]
    pub end_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "ISO 8601 duration if end time not known, e.g. PT1H30M")]
    pub duration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Event location or venue address")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Brief notes or details about the event")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "IANA timezone, e.g. America/Toronto. Omit to use default")]
    pub time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "For events repeating on a fixed pattern. A notice like 'daily 9:00-16:00 from Jul 6 to Jul 13' is ONE event on the first day with recurrence { frequency: 'daily', until: '2026-07-13' }. Null/omit for one-off events"
    )]
    pub recurrence: Option<EventRecurrence>,
    #[schemars(description = "True if this is an all-day event with no specific time")]
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Minutes before the event to send a reminder. Use for events that benefit from advance preparation (e.g. 720 for a water shutoff the night before, 1440 for a flight the day before, 60 for appointments). Omit to use the default 30-minute reminder"
    )]
    pub reminder_minutes: Option<f64>,
}

impl ExtractedEvent {
    /// A bare event for tests and adapters: everything optional unset.
    pub fn new(
        action: EventAction,
        title: impl Into<String>,
        start_date: impl Into<String>,
        all_day: bool,
    ) -> Self {
        Self {
            action,
            event_id: None,
            title: title.into(),
            start_date: start_date.into(),
            end_date: None,
            start_time: None,
            end_time: None,
            duration: None,
            location: None,
            description: None,
            time_zone: None,
            recurrence: None,
            all_day,
            reminder_minutes: None,
        }
    }
}

/// The structured output envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CalendarEventExtraction {
    pub events: Vec<ExtractedEvent>,
}
