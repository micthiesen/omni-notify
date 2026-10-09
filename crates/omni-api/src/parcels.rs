//! `GET /api/parcels`: delivery status from Omni's cached Parcel read.
//!
//! The server never calls Parcel to answer this route. It reports the last
//! scheduled read (at most every 30 minutes, every 3 hours while nothing is
//! active) and when the next one may happen.

use serde::{Deserialize, Serialize};

use crate::common::Ms;

pub const PARCELS: &str = "/api/parcels";

/// Parcel's `status_code`, named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParcelDeliveryStatus {
    /// 0: delivered.
    Completed,
    /// 1: no updates for a long time.
    Frozen,
    /// 2.
    InTransit,
    /// 3: waiting for the recipient to pick it up.
    AwaitingPickup,
    /// 4.
    OutForDelivery,
    /// 5: the carrier does not know the tracking number.
    NotFound,
    /// 6.
    FailedAttempt,
    /// 7: needs attention.
    Exception,
    /// 8: the carrier has the label information but not the package.
    InfoReceived,
    /// Any other code.
    Unknown,
}

impl ParcelDeliveryStatus {
    pub fn from_code(code: i64) -> Self {
        match code {
            0 => Self::Completed,
            1 => Self::Frozen,
            2 => Self::InTransit,
            3 => Self::AwaitingPickup,
            4 => Self::OutForDelivery,
            5 => Self::NotFound,
            6 => Self::FailedAttempt,
            7 => Self::Exception,
            8 => Self::InfoReceived,
            _ => Self::Unknown,
        }
    }

    /// Everything except a completed delivery.
    pub fn is_active(self) -> bool {
        self != Self::Completed
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Frozen => "frozen",
            Self::InTransit => "in_transit",
            Self::AwaitingPickup => "awaiting_pickup",
            Self::OutForDelivery => "out_for_delivery",
            Self::NotFound => "not_found",
            Self::FailedAttempt => "failed_attempt",
            Self::Exception => "exception",
            Self::InfoReceived => "info_received",
            Self::Unknown => "unknown",
        }
    }

    /// A short human label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "Delivered",
            Self::Frozen => "No recent updates",
            Self::InTransit => "In transit",
            Self::AwaitingPickup => "Ready for pickup",
            Self::OutForDelivery => "Out for delivery",
            Self::NotFound => "Not found",
            Self::FailedAttempt => "Delivery attempt failed",
            Self::Exception => "Exception",
            Self::InfoReceived => "Label created",
            Self::Unknown => "Unknown",
        }
    }
}

/// One carrier event, newest first in [`ParcelDelivery::events`]. Parcel
/// passes carrier text through, so `date` is verbatim and its format varies
/// by carrier (`2026-10-08 11:31:05`, `07.10.2026 15:44`, `--//--`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParcelEvent {
    pub description: String,
    pub date: Option<String>,
    pub location: Option<String>,
    pub additional: Option<String>,
}

/// The email Omni submitted this tracking number from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParcelSource {
    pub email_id: String,
    /// The `/api/email-activity/:activityId` row.
    pub activity_id: String,
    pub submitted_at: Ms,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParcelDelivery {
    pub tracking_number: String,
    pub carrier_code: String,
    /// From Parcel's carrier list; `null` when unknown.
    pub carrier_name: Option<String>,
    pub description: String,
    pub status: ParcelDeliveryStatus,
    pub status_code: i64,
    pub active: bool,
    /// Parcel's `date_expected`, verbatim (`2026-10-09 00:00:00`).
    pub expected: Option<String>,
    /// Parcel's `date_expected_end`, verbatim.
    pub expected_end: Option<String>,
    pub extra_information: Option<String>,
    /// Up to 30 events, newest first.
    pub events: Vec<ParcelEvent>,
    /// Events Parcel reported, before the 30-event cap.
    pub event_count: u32,
    /// `null` when Omni did not submit this tracking number.
    pub source: Option<ParcelSource>,
}

/// `GET /api/parcels`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParcelsResponse {
    /// `false` without `PARCEL_API_KEY`: no reads happen.
    pub configured: bool,
    /// The last successful read; `null` before the first one.
    pub fetched_at: Option<Ms>,
    pub last_attempt_at: Option<Ms>,
    /// The earliest time the scheduled read may call Parcel again.
    pub next_read_after: Option<Ms>,
    /// Set after Parcel answered 429.
    pub backoff_until: Option<Ms>,
    /// The last read's failure; `null` after a success.
    pub last_error: Option<String>,
    pub active_count: u32,
    /// Active deliveries first, then completed ones, each in Parcel's order.
    pub deliveries: Vec<ParcelDelivery>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_wire_shape() {
        let json = serde_json::json!({
            "configured": true,
            "fetchedAt": 1_790_000_000_000_i64,
            "lastAttemptAt": 1_790_000_000_000_i64,
            "nextReadAfter": 1_790_001_800_000_i64,
            "backoffUntil": null,
            "lastError": null,
            "activeCount": 1,
            "deliveries": [{
                "trackingNumber": "UUS0000000000000001",
                "carrierCode": "uniuni",
                "carrierName": "UniUni",
                "description": "Parts",
                "status": "out_for_delivery",
                "statusCode": 4,
                "active": true,
                "expected": null,
                "expectedEnd": null,
                "extraInformation": null,
                "events": [{
                    "description": "Out for delivery",
                    "date": "2026-10-08 11:31:05",
                    "location": "Springfield ST",
                    "additional": null,
                }],
                "eventCount": 1,
                "source": {"emailId": "m1", "activityId": "ParcelTracker#m1", "submittedAt": 1},
            }],
        });
        let response: ParcelsResponse = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(
            response.deliveries[0].status,
            ParcelDeliveryStatus::OutForDelivery
        );
        assert_eq!(serde_json::to_value(&response).unwrap(), json);
    }

    #[test]
    fn maps_status_codes_and_activity() {
        assert_eq!(
            ParcelDeliveryStatus::from_code(8),
            ParcelDeliveryStatus::InfoReceived
        );
        assert_eq!(
            ParcelDeliveryStatus::from_code(42),
            ParcelDeliveryStatus::Unknown
        );
        assert!(!ParcelDeliveryStatus::from_code(0).is_active());
        assert!(ParcelDeliveryStatus::from_code(7).is_active());
    }
}
