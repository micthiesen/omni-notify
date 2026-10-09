//! The primary iCloud calendar (`GET /api/calendar/status`,
//! `GET /api/calendar/events`, `GET /api/calendar/changes`).

use serde::{Deserialize, Serialize};

use crate::common::Ms;

/// `GET /api/calendar/status`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarStatusResponse {
    pub configured: bool,
    /// `ready`, `not_configured`, `identity_error` or `sync_error`.
    pub state: String,
    pub message: Option<String>,
    pub calendar_name: String,
    pub is_server_default: Option<bool>,
    pub pipeline_targets_primary: Option<bool>,
    pub writable: Option<bool>,
    pub supports_sync: Option<bool>,
    pub last_sync_at: Option<Ms>,
    pub last_full_sync_at: Option<Ms>,
    pub event_count: u64,
    /// The newest change sequence number.
    pub change_cursor: i64,
    pub default_time_zone: String,
}

/// One occurrence (a recurring series is expanded).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarOccurrence {
    pub event_id: String,
    pub recurrence_id: Option<String>,
    pub title: String,
    /// `YYYY-MM-DD` (all-day) or local `YYYY-MM-DDTHH:MM:SS`.
    pub start: String,
    pub end: String,
    pub start_utc: String,
    pub end_utc: String,
    pub all_day: bool,
    pub last_date: Option<String>,
    pub time_zone: Option<String>,
    pub location: Option<String>,
    pub recurring: bool,
    pub is_exception: bool,
    /// `none`, `organizer` or `attendee`.
    pub scheduling_role: String,
    pub free: bool,
    pub status: Option<String>,
}

/// `GET /api/calendar/events?from=&to=`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventsResponse {
    pub events: Vec<CalendarOccurrence>,
    pub from: String,
    pub to: String,
    pub truncated: bool,
    pub synced_at: Option<Ms>,
    pub stale: bool,
}

/// An event's projection before or after a change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventSnapshot {
    pub title: Option<String>,
    pub start: Option<String>,
    pub start_utc: Option<String>,
    pub all_day: bool,
    pub recurring: bool,
    pub time_zone: Option<String>,
    pub location: Option<String>,
    pub status: Option<String>,
}

/// One detected change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarChange {
    pub seq: i64,
    pub event_id: String,
    /// `created`, `updated` or `deleted`.
    pub kind: String,
    /// `omni` or `external`.
    pub origin: String,
    pub changed_fields: Vec<String>,
    pub detected_at: Ms,
    pub before: Option<CalendarEventSnapshot>,
    pub after: Option<CalendarEventSnapshot>,
}

/// `GET /api/calendar/changes?cursor=&limit=`: oldest first after `cursor`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarChangesResponse {
    pub changes: Vec<CalendarChange>,
    pub next_cursor: i64,
    pub has_more: bool,
}

/// Route paths.
pub mod paths {
    pub const STATUS: &str = "/api/calendar/status";
    pub const EVENTS: &str = "/api/calendar/events";
    pub const CHANGES: &str = "/api/calendar/changes";
}
