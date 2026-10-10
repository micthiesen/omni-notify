//! `GET /api/pets`, `GET /api/pets/health` and the finding dismissals
//! (`POST /api/pets/health/dismiss`, `POST /api/pets/health/restore`).
//!
//! Numbers are JS doubles; the backend serializes responses with
//! `JSON.stringify` semantics so integral weights render as `12`, not `12.0`.

use serde::{Deserialize, Serialize};

/// One reading, weight rounded to two decimals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeightEntry {
    /// Whisker's timestamp text, verbatim.
    pub timestamp: String,
    /// Pounds.
    pub weight: f64,
}

/// Readings per calendar day (`timestamp[0..10]`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyVisit {
    pub date: String,
    pub count: u32,
}

/// An element of the `GET /api/pets` array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pet {
    pub pet_id: String,
    pub name: String,
    /// Pounds, rounded to two decimals.
    pub current_weight: f64,
    pub weight_history: Vec<WeightEntry>,
    pub daily_visits: Vec<DailyVisit>,
}

/// `GET /api/pets`.
pub type PetsResponse = Vec<Pet>;

/// A pet health rule (also the second key part of a `pet-health-alert` row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PetHealthKind {
    /// The last 7 days' median weight is at least 3% below the 7 days ending
    /// two weeks earlier, and at least 2% below four weeks earlier.
    #[serde(rename = "weight-drop-2w")]
    WeightDrop2w,
    /// The last 7 days' median weight is at least 5% below 90 days earlier.
    #[serde(rename = "weight-drop-90d")]
    WeightDrop90d,
    /// Litter-box visits in the last 7 days are at most half the usual weekly count.
    #[serde(rename = "visit-drop")]
    VisitDrop,
    /// No reading from any pet for 48 hours (household-wide, no pet id).
    #[serde(rename = "data-gap")]
    DataGap,
}

impl PetHealthKind {
    pub const ALL: [PetHealthKind; 4] = [
        PetHealthKind::WeightDrop2w,
        PetHealthKind::WeightDrop90d,
        PetHealthKind::VisitDrop,
        PetHealthKind::DataGap,
    ];

    /// The serialized name.
    pub fn as_str(self) -> &'static str {
        match self {
            PetHealthKind::WeightDrop2w => "weight-drop-2w",
            PetHealthKind::WeightDrop90d => "weight-drop-90d",
            PetHealthKind::VisitDrop => "visit-drop",
            PetHealthKind::DataGap => "data-gap",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// One rolling 7-day block ending at `end` (oldest block first in lists).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetWeek {
    /// ISO instant (exclusive).
    pub start: String,
    /// ISO instant (inclusive).
    pub end: String,
    pub readings: u32,
    /// Median after dropping readings over 1 lb from the block median; pounds,
    /// two decimals. `null` without readings.
    pub median_weight: Option<f64>,
}

/// The last 7 days' median against the 7 days ending `weeks` weeks earlier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetWeightChange {
    pub weeks: u32,
    /// Negative is a loss; two decimals. `null` when either window has fewer
    /// than three readings.
    pub percent: Option<f64>,
    pub baseline_weight: Option<f64>,
}

/// A rule currently tripped for a pet (or household-wide for `data-gap`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetHealthFinding {
    pub kind: PetHealthKind,
    /// Percent drop, visit ratio, or hours without readings.
    pub value: f64,
    pub message: String,
    /// When this episode was dismissed in the UI; absent while it needs
    /// attention. A new episode or a later push clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_at: Option<String>,
}

/// `POST /api/pets/health/dismiss` and `POST /api/pets/health/restore`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetHealthDismissRequest {
    pub pet_id: String,
    pub kind: PetHealthKind,
}

/// The finding's dismissal after the request (`null` once restored).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetHealthDismissResponse {
    pub pet_id: String,
    pub kind: PetHealthKind,
    pub dismissed_at: Option<String>,
}

/// One pet's trend card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetTrend {
    pub pet_id: String,
    pub name: String,
    /// The last 7 days' robust median; pounds, two decimals.
    pub weight: Option<f64>,
    pub latest_reading_at: Option<String>,
    /// Oldest first.
    pub weekly: Vec<PetWeek>,
    /// 2, 4, 12 and 26 weeks.
    pub changes: Vec<PetWeightChange>,
    pub visits_last_7_days: u32,
    /// Median of the eight preceding 7-day blocks.
    pub usual_visits_per_week: Option<f64>,
    pub findings: Vec<PetHealthFinding>,
}

/// Durable alert state of one pet and rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetHealthAlertInfo {
    /// `null` for household-wide rules.
    pub pet_id: Option<String>,
    pub kind: PetHealthKind,
    /// The rule is currently tripped.
    pub active: bool,
    pub last_notified_at: Option<String>,
    pub last_message: Option<String>,
    pub recovered_at: Option<String>,
}

/// `GET /api/pets/health` (and the `pets_read` `trend` resource).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetHealthResponse {
    pub generated_at: String,
    /// Newest reading of any pet.
    pub latest_reading_at: Option<String>,
    pub hours_since_latest_reading: Option<f64>,
    /// The household-wide `data-gap` finding, when tripped.
    pub data_gap: Option<PetHealthFinding>,
    pub pets: Vec<PetTrend>,
    /// Newest notification first.
    pub alerts: Vec<PetHealthAlertInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_wire_shape() {
        let json = serde_json::json!([{
            "petId": "PET-1",
            "name": "Sam",
            "currentWeight": 13.95,
            "weightHistory": [{"timestamp": "2026-03-20T15:37:39", "weight": 11.99}],
            "dailyVisits": [{"date": "2026-03-20", "count": 2}],
        }]);
        let pets: PetsResponse = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(pets[0].daily_visits[0].count, 2);
        assert_eq!(serde_json::to_value(&pets).unwrap(), json);
    }

    #[test]
    fn round_trips_the_health_shape() {
        let json = serde_json::json!({
            "generatedAt": "2026-10-09T12:00:00.000Z",
            "latestReadingAt": "2026-10-09T04:27:58.000Z",
            "hoursSinceLatestReading": 7.5,
            "dataGap": null,
            "pets": [{
                "petId": "PET-1",
                "name": "Sam",
                "weight": 13.41,
                "latestReadingAt": "2026-10-09T04:27:58.000Z",
                "weekly": [{
                    "start": "2026-10-02T12:00:00.000Z",
                    "end": "2026-10-09T12:00:00.000Z",
                    "readings": 9,
                    "medianWeight": 13.41
                }],
                "changes": [{"weeks": 2, "percent": -3.03, "baselineWeight": 13.83}],
                "visitsLast7Days": 9,
                "usualVisitsPerWeek": 22.5,
                "findings": [{
                    "kind": "weight-drop-2w",
                    "value": 3.03,
                    "message": "Sam: 13.41 lb"
                }, {
                    "kind": "weight-drop-90d",
                    "value": 5.2,
                    "message": "Sam: 13.41 lb",
                    "dismissedAt": "2026-10-09T11:00:00.000Z"
                }]
            }],
            "alerts": [{
                "petId": null,
                "kind": "data-gap",
                "active": false,
                "lastNotifiedAt": "2026-10-02T09:00:00.000Z",
                "lastMessage": "No readings",
                "recoveredAt": null
            }]
        });
        let health: PetHealthResponse = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(health.alerts[0].kind, PetHealthKind::DataGap);
        assert_eq!(serde_json::to_value(&health).unwrap(), json);
        for kind in PetHealthKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
            assert_eq!(PetHealthKind::parse(kind.as_str()), Some(kind));
        }
    }
}
