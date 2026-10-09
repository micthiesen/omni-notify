//! `PetTracker`: every ten minutes, sync Whisker
//! pets and new scale readings into the relational tables.

use std::sync::Arc;

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_core::js::{date_parse, math_round, to_iso_string};
use omni_tasks::{CronSchedule, InvalidScheduleError, RunContext, Task, TaskError, TaskOptions};

use super::api::{WhiskerApi, WhiskerApiError};
use super::auth::{WhiskerAuth, WhiskerAuthenticationError};
use super::math::{Point, linear_regression};
use super::persistence::{PetRow, PetStore, WeightHistoryRow};

pub const TASK_NAME: &str = "PetTracker";
pub const SCHEDULE: &str = "0 */10 * * * *";
const MS_PER_DAY: f64 = 86_400_000.0;
const LOG: &str = "PetTracker";

/// Whisker's profiles have Sam and Sandy reversed; their weight trends match
/// the swapped names. Correct the labels here rather than in the Whisker app.
const PET_NAME_OVERRIDES: [(&str, &str); 2] = [
    ("PET-b4738d2e-9a37-4d70-b401-a86e56bfd180", "Sandy"),
    ("PET-697f1644-6b4b-43cb-945b-61426edcbb86", "Sam"),
];

/// The name shown for a Whisker pet.
pub fn pet_display_name<'a>(pet_id: &str, whisker_name: &'a str) -> &'a str {
    PET_NAME_OVERRIDES
        .iter()
        .find(|(id, _)| *id == pet_id)
        .map_or(whisker_name, |(_, name)| name)
}

/// `(Math.round(value * 10^d) / 10^d).toFixed(d)`.
pub fn round_fixed(value: f64, decimals: i32) -> String {
    let factor = 10f64.powi(decimals);
    let rounded = math_round(value * factor) / factor;
    // `toFixed` never prints a negative zero.
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    format!(
        "{rounded:.prec$}",
        prec = usize::try_from(decimals).unwrap_or(0)
    )
}

#[derive(Debug, thiserror::Error)]
pub enum PetSyncError {
    #[error(transparent)]
    Auth(#[from] WhiskerAuthenticationError),
    #[error(transparent)]
    Api(#[from] WhiskerApiError),
    #[error(transparent)]
    Store(#[from] omni_store::StoreError),
}

struct PetSyncResult {
    name: String,
    pet_id: String,
    current_weight: f64,
}

/// The pet sync task.
pub struct PetTrackerTask {
    schedule: CronSchedule,
    auth: Arc<WhiskerAuth>,
    api: WhiskerApi,
    pets: PetStore,
    clock: SharedClock,
    tz: TimeZone,
}

impl PetTrackerTask {
    pub fn new(
        auth: Arc<WhiskerAuth>,
        api: WhiskerApi,
        pets: PetStore,
        clock: SharedClock,
        tz: TimeZone,
    ) -> Result<Self, InvalidScheduleError> {
        Ok(Self {
            schedule: CronSchedule::parse(SCHEDULE, &tz)?,
            auth,
            api,
            pets,
            clock,
            tz,
        })
    }

    /// One sync pass.
    pub async fn sync(&self) -> Result<(), PetSyncError> {
        let session = self.auth.authenticate().await?;
        let pets = self
            .api
            .fetch_pets_by_user(&session.id_token, &session.user_id)
            .await?;
        let now = to_iso_string(self.clock.now_ms());
        let mut affected: Vec<PetSyncResult> = Vec::new();
        let mut total_new = 0usize;
        for pet in &pets {
            let name = pet_display_name(&pet.pet_id, &pet.name).to_owned();
            self.pets
                .upsert_pet(&PetRow {
                    pet_id: pet.pet_id.clone(),
                    name: name.clone(),
                    current_weight: pet.weight,
                    updated_at: now.clone(),
                })
                .await?;
            let mut new_readings = 0usize;
            for reading in &pet.weight_history {
                let inserted = self
                    .pets
                    .insert_weight_reading(&WeightHistoryRow {
                        pet_id: pet.pet_id.clone(),
                        timestamp: reading.timestamp.clone(),
                        weight: reading.weight,
                    })
                    .await?;
                if inserted {
                    new_readings += 1;
                }
            }
            total_new += new_readings;
            if new_readings > 0 {
                affected.push(PetSyncResult {
                    name,
                    pet_id: pet.pet_id.clone(),
                    current_weight: pet.weight,
                });
            }
        }
        let total: usize = pets.iter().map(|p| p.weight_history.len()).sum();
        let message = format!(
            "Synced {} pets, {total_new} new / {total} total readings",
            pets.len()
        );
        if total_new > 0 {
            tracing::info!(target: LOG, "{message}");
        } else {
            tracing::debug!(target: LOG, "{message}");
        }
        if !affected.is_empty() {
            let mut lines = Vec::with_capacity(affected.len());
            for pet in &affected {
                lines.push(self.format_pet_line(pet).await?);
            }
            tracing::info!(target: LOG, "{}", lines.join(", "));
        }
        Ok(())
    }

    async fn format_pet_line(&self, pet: &PetSyncResult) -> Result<String, PetSyncError> {
        let history = self
            .pets
            .recent_weight_history(&pet.pet_id, 30, self.clock.now_ms())
            .await?;
        let weight = format!("{} lbs", round_fixed(pet.current_weight, 1));
        if history.len() < 2 {
            return Ok(format!("{}: {weight}", pet.name));
        }
        let at = |row: &WeightHistoryRow| date_parse(&row.timestamp, &self.tz);
        #[allow(clippy::cast_precision_loss)]
        let points: Vec<Point> = match at(&history[0]) {
            Some(t0) => history
                .iter()
                .map(|row| Point {
                    x: at(row).map_or(f64::NAN, |t| (t - t0) as f64 / MS_PER_DAY),
                    y: row.weight,
                })
                .collect(),
            None => history
                .iter()
                .map(|row| Point {
                    x: f64::NAN,
                    y: row.weight,
                })
                .collect(),
        };
        let fit = linear_regression(&points);
        let per_week = fit.slope * 7.0;
        let sign = if per_week >= 0.0 { "+" } else { "" };
        let trend = format!("{sign}{} lbs/wk", round_fixed(per_week, 2));
        let qualifier = if fit.r2 < 0.3 { ", weak trend" } else { "" };
        Ok(format!("{}: {weight} ({trend}{qualifier})", pet.name))
    }
}

impl Task for PetTrackerTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions {
            jitter: std::time::Duration::ZERO,
            run_on_startup: true,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move { self.sync().await.map_err(TaskError::from_error) })
    }
}
