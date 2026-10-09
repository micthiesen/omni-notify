//! `calendar.event_changed` publications from the durable change feed.
//!
//! Each sync commits its change rows before anything is published; this pass
//! then hands rows after `SyncState::published_seq` to the MCP Events port and
//! advances that cursor. A crash between publishing and advancing replays the
//! same rows, and the dedup key (eventId plus the resulting ETag, or the last
//! ETag for a deletion) keeps their event IDs, so nothing is delivered twice.
//! Without a subscriber (or with MCP Events disabled) the cursor advances
//! without publishing: changes are news, not a backlog.

use std::collections::{BTreeMap, BTreeSet};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use omni_api::events::{
    CALENDAR_EVENT_CHANGED, CalendarChangeKind, CalendarChangeOrigin, CalendarEventChanged,
    TITLE_LIMIT, bounded_title, bounded_title_flagged,
};
use omni_runtime::ports::{EventPublication, PortError};
use omni_store::entity::{EntityOps as _, EntityWrite as _, UpsertOpts};
use serde_json::Value;

use super::ics::IcsDoc;
use super::store::{ChangeKind, ChangeOrigin, ChangeRow, MirrorResource, SINGLETON, SyncState};
use super::time::{Zones, rfc3339};
use super::{PrimaryCalendar, PrimaryError, expand, model};

const LOG: &str = "CalendarPrimary";

/// Change rows detected longer ago than this are not published (a pass that
/// kept failing for a day would otherwise deliver history).
pub const CHANGE_STALE_MS: i64 = 24 * 60 * 60 * 1000;
/// How far ahead the next occurrence of a changed series is looked up.
const NEXT_OCCURRENCE_HORIZON: SignedDuration = SignedDuration::from_hours(24 * 400);

pub(crate) fn port_error(error: PortError) -> PrimaryError {
    PrimaryError::Transport {
        message: format!("MCP Events: {error}"),
        transient: true,
    }
}

/// Whether any MCP Events subscription to `name` is active; `false` when the
/// port is unset.
pub async fn subscribed(service: &PrimaryCalendar, name: &str) -> Result<bool, PrimaryError> {
    match service.ports().event_publisher() {
        None => Ok(false),
        Some(publisher) => publisher
            .active_arguments(name)
            .await
            .map(|args| !args.is_empty())
            .map_err(port_error),
    }
}

/// The dedup key of a change: the resulting version, or the last one for a
/// deletion. A resource recreated at the same href has a new ETag.
pub fn change_dedup_key(row: &ChangeRow) -> String {
    match (&row.kind, &row.etag) {
        (ChangeKind::Deleted, _) | (_, None) => format!(
            "{}:deleted:{}",
            row.event_id,
            row.previous_etag.as_deref().unwrap_or_default()
        ),
        (_, Some(etag)) => format!("{}:{etag}", row.event_id),
    }
}

/// The publication for one change row. `mirror` is the current mirror row of
/// the resource; its text is used (full summary, next occurrence) only when it
/// still holds the version the change produced.
pub fn change_publication(
    row: &ChangeRow,
    mirror: Option<&MirrorResource>,
    default_tz: &TimeZone,
) -> Option<EventPublication> {
    let projection = row.after.as_ref().or(row.before.as_ref());
    let detected = Timestamp::from_millisecond(row.detected_at).ok()?;
    let current = mirror
        .filter(|m| row.kind != ChangeKind::Deleted && row.etag.as_deref() == Some(&m.etag))
        .and_then(|m| m.ics.as_deref())
        .and_then(|text| IcsDoc::parse(text).ok());
    let (summary, summary_truncated) = match &current {
        Some(doc) => model::primary_event(doc)
            .and_then(|e| e.summary())
            .map_or((None, false), |s| bounded_title_flagged(&s)),
        // Projections keep 200 characters, so a full-length one may be cut.
        None => projection
            .and_then(|p| p.summary.as_deref())
            .map_or((None, false), |s| {
                (bounded_title(s), s.chars().count() >= TITLE_LIMIT)
            }),
    };
    let next_start = current.as_ref().and_then(|doc| {
        let zones = Zones::new(doc, default_tz.clone());
        let until = detected.checked_add(NEXT_OCCURRENCE_HORIZON).ok()?;
        expand::expand(doc, &zones, &BTreeSet::new(), detected, until)
            .occurrences
            .into_iter()
            .find(|o| o.start_utc >= detected && !o.status.as_deref().is_some_and(is_cancelled))
            .map(|o| rfc3339(o.start_utc))
    });
    let payload = CalendarEventChanged {
        event_id: row.event_id.clone(),
        uid: row.uid.clone(),
        change_kind: match row.kind {
            ChangeKind::Created => CalendarChangeKind::Created,
            ChangeKind::Updated => CalendarChangeKind::Updated,
            ChangeKind::Deleted => CalendarChangeKind::Deleted,
        },
        summary,
        summary_truncated,
        start: next_start.or_else(|| projection.and_then(|p| p.start_utc.clone())),
        all_day: projection.is_some_and(|p| p.all_day),
        recurring: projection.is_some_and(|p| p.recurring),
        changed_fields: match row.kind {
            ChangeKind::Updated => row.changed_fields.iter().take(13).cloned().collect(),
            ChangeKind::Created | ChangeKind::Deleted => Vec::new(),
        },
        version: match row.kind {
            ChangeKind::Deleted => None,
            _ => row.etag.clone(),
        },
        origin: match row.origin {
            ChangeOrigin::Omni => CalendarChangeOrigin::Omni,
            ChangeOrigin::External => CalendarChangeOrigin::External,
        },
        detected_at: omni_core::js::to_iso_string(row.detected_at),
    };
    let Ok(Value::Object(data)) = serde_json::to_value(payload) else {
        return None;
    };
    Some(EventPublication {
        name: CALENDAR_EVENT_CHANGED,
        dedup_key: change_dedup_key(row),
        occurred_at_ms: row.detected_at,
        data,
    })
}

fn is_cancelled(status: &str) -> bool {
    status.eq_ignore_ascii_case("cancelled")
}

async fn advance(service: &PrimaryCalendar, seq: i64) -> Result<(), PrimaryError> {
    service
        .store()
        .write(move |tx| {
            let Some(mut state) = tx.get::<SyncState>(&SINGLETON.to_owned())? else {
                return Ok(());
            };
            if state.published_seq >= seq {
                return Ok(());
            }
            state.published_seq = seq;
            tx.upsert(&state, UpsertOpts::default())
        })
        .await
        .map_err(PrimaryError::store)
}

/// Publishes change rows after the published cursor; returns how many were
/// handed to the port. Runs outside the sync lock; overlapping passes only
/// replay dedup keys the outbox drops.
pub async fn publish_changes(service: &PrimaryCalendar) -> Result<usize, PrimaryError> {
    let state = service
        .store()
        .read(|docs| docs.get::<SyncState>(&SINGLETON.to_owned()))
        .await?;
    let Some(state) = state else {
        return Ok(0);
    };
    if state.published_seq >= state.change_seq {
        return Ok(0);
    }
    let target = state.change_seq;
    let publisher = service.ports().event_publisher();
    let subscribed = match &publisher {
        None => false,
        Some(publisher) => !publisher
            .active_arguments(CALENDAR_EVENT_CHANGED)
            .await
            .map_err(port_error)?
            .is_empty(),
    };
    let Some(publisher) = publisher.filter(|_| subscribed) else {
        advance(service, target).await?;
        return Ok(0);
    };
    let after = state.published_seq;
    let (mut rows, mirror) = service
        .store()
        .read(|docs| {
            Ok((
                docs.get_all::<ChangeRow>()?,
                docs.get_all::<MirrorResource>()?,
            ))
        })
        .await?;
    rows.retain(|r| r.seq > after && r.seq <= target);
    rows.sort_by_key(|r| r.seq);
    let mirror: BTreeMap<String, MirrorResource> = mirror
        .into_iter()
        .map(|m| (m.event_id.clone(), m))
        .collect();
    let stale_before = service.now_ms() - CHANGE_STALE_MS;
    let mut published = 0;
    for row in &rows {
        if row.detected_at < stale_before {
            continue;
        }
        let Some(event) = change_publication(row, mirror.get(&row.event_id), service.default_tz())
        else {
            continue;
        };
        match publisher.publish(&event).await {
            Ok(_) => published += 1,
            Err(PortError::Failed {
                transient: false,
                message,
            }) => {
                // A rejected payload can never succeed; skip it rather than
                // block every later change.
                tracing::warn!(
                    target: LOG,
                    "Calendar change {} rejected by MCP Events: {message}",
                    row.seq
                );
            }
            Err(error) => {
                advance(service, row.seq - 1).await?;
                return Err(port_error(error));
            }
        }
    }
    advance(service, target).await?;
    Ok(published)
}
