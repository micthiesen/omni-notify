//! The relational `pets` and `pet_weight_history` tables,
//! with mitools' exact DDL.

use std::collections::HashMap;

use omni_core::js::{locale_compare, to_iso_string, utf16_slice};
pub use omni_store::table::pets::{PetRow, WeightHistoryRow};
use omni_store::table::{SqlValue, Table};
use omni_store::{Store, StoreError};

const DAY_MS: i64 = 86_400_000;

/// A pet with its whole weight history (oldest first).
#[derive(Clone, Debug, PartialEq)]
pub struct PetWithHistory {
    pub pet: PetRow,
    pub weight_history: Vec<WeightHistoryRow>,
}

/// Visits per calendar day (`timestamp[0..10]`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DailyVisitCount {
    pub date: String,
    pub count: u32,
}

/// Both pet tables.
#[derive(Clone)]
pub struct PetStore {
    pets: Table<PetRow>,
    history: Table<WeightHistoryRow>,
}

impl PetStore {
    /// Opens (creating if needed) both tables.
    pub async fn open(store: &Store) -> Result<Self, StoreError> {
        Ok(Self {
            pets: store.table::<PetRow>().await?,
            history: store.table::<WeightHistoryRow>().await?,
        })
    }

    /// `INSERT OR REPLACE` a pet.
    pub async fn upsert_pet(&self, pet: &PetRow) -> Result<(), StoreError> {
        self.pets.upsert(pet).await
    }

    /// `INSERT OR IGNORE` a reading; `true` when it is new.
    pub async fn insert_weight_reading(&self, row: &WeightHistoryRow) -> Result<bool, StoreError> {
        self.history.insert(row).await
    }

    pub async fn get_pet(&self, pet_id: &str) -> Result<Option<PetRow>, StoreError> {
        Ok(self
            .pets
            .query("pet_id = ?", vec![SqlValue::Text(pet_id.to_owned())])
            .await?
            .into_iter()
            .next())
    }

    pub async fn all_pets(&self) -> Result<Vec<PetRow>, StoreError> {
        self.pets.all().await
    }

    /// Readings oldest first.
    pub async fn weight_history(&self, pet_id: &str) -> Result<Vec<WeightHistoryRow>, StoreError> {
        self.history
            .query(
                "pet_id = ? ORDER BY timestamp ASC",
                vec![SqlValue::Text(pet_id.to_owned())],
            )
            .await
    }

    /// Readings whose timestamp text sorts at or after `now - days`
    /// (an ISO string comparison, as TS does).
    pub async fn recent_weight_history(
        &self,
        pet_id: &str,
        days: i64,
        now: i64,
    ) -> Result<Vec<WeightHistoryRow>, StoreError> {
        let cutoff = to_iso_string(now - days * DAY_MS);
        self.history
            .query(
                "pet_id = ? AND timestamp >= ? ORDER BY timestamp ASC",
                vec![SqlValue::Text(pet_id.to_owned()), SqlValue::Text(cutoff)],
            )
            .await
    }

    pub async fn all_pets_with_history(&self) -> Result<Vec<PetWithHistory>, StoreError> {
        let mut out = Vec::new();
        for pet in self.all_pets().await? {
            let weight_history = self.weight_history(&pet.pet_id).await?;
            out.push(PetWithHistory {
                pet,
                weight_history,
            });
        }
        Ok(out)
    }

    pub async fn daily_visit_counts(
        &self,
        pet_id: &str,
    ) -> Result<Vec<DailyVisitCount>, StoreError> {
        Ok(daily_visit_counts(&self.weight_history(pet_id).await?))
    }

    /// Deletes every reading and pet (seed tool).
    pub async fn clear_all(&self) -> Result<(), StoreError> {
        self.history.clear().await?;
        self.pets.clear().await
    }
}

/// Counts readings per `timestamp[0..10]`, sorted with `localeCompare`.
pub fn daily_visit_counts(history: &[WeightHistoryRow]) -> Vec<DailyVisitCount> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: HashMap<String, u32> = HashMap::new();
    for row in history {
        let date = utf16_slice(&row.timestamp, 0, 10).into_owned();
        let entry = counts.entry(date.clone()).or_insert_with(|| {
            order.push(date);
            0
        });
        *entry += 1;
    }
    let mut out: Vec<DailyVisitCount> = order
        .into_iter()
        .map(|date| {
            let count = counts.get(&date).copied().unwrap_or(0);
            DailyVisitCount { date, count }
        })
        .collect();
    out.sort_by(|a, b| locale_compare(&a.date, &b.date));
    out
}
