//! `GET /api/calendar/status`, `/api/calendar/events` and
//! `/api/calendar/changes` over the primary calendar.

use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use omni_api::calendar::{
    CalendarChange, CalendarChangesResponse, CalendarEventSnapshot, CalendarEventsResponse,
    CalendarOccurrence, CalendarStatusResponse, paths,
};
use omni_server_kit::{ApiError, ApiResult};
use serde::Deserialize;

use crate::mcp::{occurrence_view, window};
use crate::primary::model::Projection;
use crate::primary::store::{ChangeKind, ChangeOrigin};
use crate::primary::{PrimaryCalendar, PrimaryError, READ_MAX_AGE_MS};

const MAX_EVENTS: usize = 1_000;
const MAX_CHANGES: usize = 500;

pub fn router(service: PrimaryCalendar) -> Router {
    Router::new()
        .route(paths::STATUS, get(status))
        .route(paths::EVENTS, get(events))
        .route(paths::CHANGES, get(changes))
        .with_state(service)
}

fn api_error(error: &PrimaryError) -> ApiError {
    match error.code() {
        "invalid_input" => ApiError::bad_request(error.to_string()),
        "not_configured" => ApiError::not_found(error.to_string()),
        _ => ApiError::internal(error.tool_text()),
    }
}

async fn status(State(service): State<PrimaryCalendar>) -> ApiResult<CalendarStatusResponse> {
    let s = service.status(false).await;
    Ok(axum::Json(CalendarStatusResponse {
        configured: s.configured,
        state: s.state.to_owned(),
        message: s.message,
        calendar_name: crate::primary::identity::PRIMARY_NAME.to_owned(),
        is_server_default: s.is_server_default,
        pipeline_targets_primary: s.pipeline_targets_primary,
        writable: s.writable,
        supports_sync: s.supports_sync,
        last_sync_at: s.last_sync_at,
        last_full_sync_at: s.last_full_sync_at,
        event_count: s.event_count,
        change_cursor: s.change_cursor,
        default_time_zone: service.default_tz_name().to_owned(),
    }))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    from: Option<String>,
    to: Option<String>,
}

async fn events(
    State(service): State<PrimaryCalendar>,
    Query(query): Query<EventsQuery>,
) -> ApiResult<CalendarEventsResponse> {
    let (from, to) = window(&service, query.from.as_deref(), query.to.as_deref(), 0, 14)
        .map_err(|e| ApiError::bad_request(e.message))?;
    let freshness = service
        .ensure_fresh(READ_MAX_AGE_MS)
        .await
        .map_err(|e| api_error(&e))?;
    let (found, truncated) = service
        .occurrences(from, to)
        .await
        .map_err(|e| api_error(&e))?;
    let more = found.len() > MAX_EVENTS;
    let events = found
        .iter()
        .take(MAX_EVENTS)
        .map(|f| {
            let o = occurrence_view(&f.event_id, &f.occurrence);
            CalendarOccurrence {
                event_id: o.event_id,
                recurrence_id: o.recurrence_id,
                title: o.title,
                start: o.start,
                end: o.end,
                start_utc: o.start_utc,
                end_utc: o.end_utc,
                all_day: o.all_day,
                last_date: o.last_date,
                time_zone: o.time_zone,
                location: o.location,
                recurring: o.recurring,
                is_exception: o.is_exception,
                scheduling_role: o.scheduling_role,
                free: o.free,
                status: o.status,
            }
        })
        .collect();
    Ok(axum::Json(CalendarEventsResponse {
        events,
        from: crate::primary::time::rfc3339(from),
        to: crate::primary::time::rfc3339(to),
        truncated: truncated || more,
        synced_at: freshness.synced_at,
        stale: freshness.stale,
    }))
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    cursor: Option<i64>,
    limit: Option<usize>,
}

fn snapshot(p: &Projection) -> CalendarEventSnapshot {
    CalendarEventSnapshot {
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

async fn changes(
    State(service): State<PrimaryCalendar>,
    Query(query): Query<ChangesQuery>,
) -> ApiResult<CalendarChangesResponse> {
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_CHANGES);
    let (rows, next_cursor, has_more) = service
        .changes_since(query.cursor, limit, None)
        .await
        .map_err(|e| api_error(&e))?;
    Ok(axum::Json(CalendarChangesResponse {
        changes: rows
            .iter()
            .map(|r| CalendarChange {
                seq: r.seq,
                event_id: r.event_id.clone(),
                kind: match r.kind {
                    ChangeKind::Created => "created",
                    ChangeKind::Updated => "updated",
                    ChangeKind::Deleted => "deleted",
                }
                .to_owned(),
                origin: match r.origin {
                    ChangeOrigin::Omni => "omni",
                    ChangeOrigin::External => "external",
                }
                .to_owned(),
                changed_fields: r.changed_fields.clone(),
                detected_at: r.detected_at,
                before: r.before.as_ref().map(snapshot),
                after: r.after.as_ref().map(snapshot),
            })
            .collect(),
        next_cursor,
        has_more,
    }))
}
