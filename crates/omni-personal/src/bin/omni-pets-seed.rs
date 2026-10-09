//! Dev tool (`src/pet-tracker/seed.ts`): replaces the pet tables in
//! `./docstore.db` with 90 days of plausible readings for three fake pets.
//! Never run it against production data.

use std::path::Path;
use std::sync::Arc;

use jiff::tz::TimeZone;
use jiff::{Span, Zoned};
use omni_core::clock::{SharedClock, SystemClock};
use omni_core::js::to_iso_string;
use omni_personal::js::math_round;
use omni_personal::pets::persistence::{PetRow, PetStore, WeightHistoryRow};
use omni_store::{Store, StoreOptions};

struct PetSeed {
    pet_id: &'static str,
    name: &'static str,
    baseline_weight: f64,
    trend_per_day: f64,
}

const PETS: [PetSeed; 3] = [
    PetSeed {
        pet_id: "seed-luna",
        name: "Luna",
        baseline_weight: 9.5,
        trend_per_day: 0.5 / 90.0,
    },
    PetSeed {
        pet_id: "seed-mochi",
        name: "Mochi",
        baseline_weight: 11.2,
        trend_per_day: -0.3 / 90.0,
    },
    PetSeed {
        pet_id: "seed-pepper",
        name: "Pepper",
        baseline_weight: 7.8,
        trend_per_day: 0.2 / 90.0,
    },
];
const DAYS: i64 = 90;
const MIN_READINGS_PER_DAY: u32 = 3;
const MAX_READINGS_PER_DAY: u32 = 5;
const FLUCTUATION: f64 = 0.3;

fn reading_times(day: &Zoned, count: u32) -> Result<Vec<Zoned>, jiff::Error> {
    let mut times = Vec::new();
    for _ in 0..count {
        let hour: i8 = rand::random_range(6..=23);
        let minute: i8 = rand::random_range(0..=59);
        let second: i8 = rand::random_range(0..=59);
        times.push(
            day.with()
                .hour(hour)
                .minute(minute)
                .second(second)
                .subsec_nanosecond(0)
                .build()?,
        );
    }
    times.sort();
    Ok(times)
}

async fn seed() -> Result<(), Box<dyn std::error::Error>> {
    let clock: SharedClock = Arc::new(SystemClock);
    let store = Store::open(Path::new("docstore.db"), StoreOptions::new(clock.clone())).await?;
    let pets = PetStore::open(&store).await?;
    pets.clear_all().await?;
    let now = Zoned::now().with_time_zone(TimeZone::system());
    let now_iso = to_iso_string(clock.now_ms());
    let mut total = 0usize;
    for pet in &PETS {
        #[allow(clippy::cast_precision_loss)]
        let final_weight = pet.baseline_weight + pet.trend_per_day * DAYS as f64;
        for day_offset in (0..DAYS).rev() {
            let day = now
                .checked_sub(Span::new().days(day_offset))?
                .start_of_day()?;
            #[allow(clippy::cast_precision_loss)]
            let trend_weight = pet.baseline_weight + pet.trend_per_day * (DAYS - day_offset) as f64;
            let count = rand::random_range(MIN_READINGS_PER_DAY..=MAX_READINGS_PER_DAY);
            for at in reading_times(&day, count)? {
                let fluctuation: f64 = rand::random_range(-FLUCTUATION..FLUCTUATION);
                let weight = math_round((trend_weight + fluctuation) * 100.0) / 100.0;
                pets.insert_weight_reading(&WeightHistoryRow {
                    pet_id: pet.pet_id.to_owned(),
                    timestamp: to_iso_string(at.timestamp().as_millisecond()),
                    weight,
                })
                .await?;
                total += 1;
            }
        }
        pets.upsert_pet(&PetRow {
            pet_id: pet.pet_id.to_owned(),
            name: pet.name.to_owned(),
            current_weight: math_round(final_weight * 100.0) / 100.0,
            updated_at: now_iso.clone(),
        })
        .await?;
    }
    tracing::info!(
        target: "PetSeed",
        "Seeded {} pets with {total} weight readings ({DAYS} days)",
        PETS.len()
    );
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    match seed().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(target: "PetSeed", "Seeding failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
