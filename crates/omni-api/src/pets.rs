//! Owned by WP13: `GET /api/pets`.
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
}
