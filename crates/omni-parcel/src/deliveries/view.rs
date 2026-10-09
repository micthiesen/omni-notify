//! The cached deliveries as `omni-api` DTOs, joined with the tracking numbers
//! Omni submitted. Pure; never calls Parcel.

use std::collections::HashMap;

use omni_api::email::EmailPipelineName;
use omni_api::parcels::{ParcelDelivery, ParcelEvent, ParcelSource, ParcelsResponse};
use omni_email::activity::activity_id;

use super::state::{CachedDelivery, DeliveriesSnapshot, ReadState, next_due, wants_active_cadence};
use crate::persistence::{SubmissionStatus, SubmittedDelivery};

/// Tracking numbers compare without whitespace and case.
pub fn normalize_tracking(number: &str) -> String {
    number
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_uppercase)
        .collect()
}

fn source_index(submissions: &[SubmittedDelivery]) -> HashMap<String, ParcelSource> {
    submissions
        .iter()
        .filter(|row| row.status != Some(SubmissionStatus::Rejected))
        .map(|row| {
            (
                normalize_tracking(&row.tracking_number),
                ParcelSource {
                    email_id: row.email_id.clone(),
                    activity_id: activity_id(EmailPipelineName::ParcelTracker, &row.email_id),
                    submitted_at: row.submitted_at,
                },
            )
        })
        .collect()
}

fn to_dto(delivery: &CachedDelivery, sources: &HashMap<String, ParcelSource>) -> ParcelDelivery {
    let status = delivery.status();
    ParcelDelivery {
        tracking_number: delivery.tracking_number.clone(),
        carrier_code: delivery.carrier_code.clone(),
        carrier_name: delivery.carrier_name.clone(),
        description: delivery.description.clone(),
        status,
        status_code: delivery.status_code,
        active: status.is_active(),
        expected: delivery.expected.clone(),
        expected_end: delivery.expected_end.clone(),
        extra_information: delivery.extra_information.clone(),
        events: delivery
            .events
            .iter()
            .map(|event| ParcelEvent {
                description: event.description.clone(),
                date: event.date.clone(),
                location: event.location.clone(),
                additional: event.additional.clone(),
            })
            .collect(),
        event_count: delivery.event_count,
        source: sources
            .get(&normalize_tracking(&delivery.tracking_number))
            .cloned(),
    }
}

/// `GET /api/parcels` (and the MCP tools' source data).
pub fn response(
    configured: bool,
    snapshot: Option<&DeliveriesSnapshot>,
    state: Option<&ReadState>,
    submissions: &[SubmittedDelivery],
) -> ParcelsResponse {
    let sources = source_index(submissions);
    let mut deliveries: Vec<ParcelDelivery> = snapshot
        .map(|s| s.deliveries.iter().map(|d| to_dto(d, &sources)).collect())
        .unwrap_or_default();
    // Stable: active first, Parcel's order within each group.
    deliveries.sort_by_key(|d| !d.active);
    let active_count = deliveries.iter().filter(|d| d.active).count();
    let active = wants_active_cadence(snapshot, submissions);
    ParcelsResponse {
        configured,
        fetched_at: snapshot.map(|s| s.fetched_at),
        last_attempt_at: state.and_then(|s| s.last_attempt_at),
        next_read_after: if configured {
            state.and_then(|s| next_due(s, active))
        } else {
            None
        },
        backoff_until: state.and_then(|s| s.backoff_until),
        last_error: state.and_then(|s| s.last_error.clone()),
        active_count: u32::try_from(active_count).unwrap_or(u32::MAX),
        deliveries,
    }
}

/// The cached delivery for `tracking_number` (whitespace and case ignored).
pub fn find<'a>(
    response: &'a ParcelsResponse,
    tracking_number: &str,
) -> Option<&'a ParcelDelivery> {
    let wanted = normalize_tracking(tracking_number);
    response
        .deliveries
        .iter()
        .find(|d| normalize_tracking(&d.tracking_number) == wanted)
}

/// The submission for `tracking_number`, when Omni sent one.
pub fn submitted_source(
    submissions: &[SubmittedDelivery],
    tracking_number: &str,
) -> Option<ParcelSource> {
    source_index(submissions).remove(&normalize_tracking(tracking_number))
}
