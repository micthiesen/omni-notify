//! `GET /api/pets` and `GET /api/pets/:petId/export.csv`.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use jiff::tz::TimeZone;
use omni_api::pets::{DailyVisit, Pet, WeightEntry};
use omni_core::clock::SharedClock;
use omni_core::js::{
    MAX_DATE_MS, date_parse, json_stringify, math_round, number_to_string, string_to_number,
};
use omni_server_kit::{ApiError, json_response};
use serde::Deserialize;

use super::persistence::PetStore;

#[derive(Clone)]
struct PetsState {
    pets: PetStore,
    clock: SharedClock,
    tz: TimeZone,
}

/// `Math.round(n * 100) / 100`.
fn round2(n: f64) -> f64 {
    math_round(n * 100.0) / 100.0
}

/// The pet routes, state applied.
pub fn router(pets: PetStore, clock: SharedClock, tz: TimeZone) -> Router {
    Router::new()
        .route("/api/pets", get(list_pets))
        .route("/api/pets/{pet_id}/export.csv", get(export_csv))
        .with_state(PetsState { pets, clock, tz })
}

async fn list_pets(State(state): State<PetsState>) -> Result<Response, ApiError> {
    let pets = state
        .pets
        .all_pets_with_history()
        .await
        .map_err(ApiError::internal)?;
    let mut response: Vec<Pet> = Vec::with_capacity(pets.len());
    for entry in pets {
        let daily_visits = state
            .pets
            .daily_visit_counts(&entry.pet.pet_id)
            .await
            .map_err(ApiError::internal)?;
        response.push(Pet {
            pet_id: entry.pet.pet_id,
            name: entry.pet.name,
            current_weight: round2(entry.pet.current_weight),
            weight_history: entry
                .weight_history
                .into_iter()
                .map(|row| WeightEntry {
                    timestamp: row.timestamp,
                    weight: round2(row.weight),
                })
                .collect(),
            daily_visits: daily_visits
                .into_iter()
                .map(|visit| DailyVisit {
                    date: visit.date,
                    count: visit.count,
                })
                .collect(),
        });
    }
    let value = serde_json::to_value(&response).map_err(ApiError::internal)?;
    Ok(json_response(StatusCode::OK, json_stringify(&value)))
}

#[derive(Deserialize)]
struct ExportQuery {
    days: Option<String>,
}

async fn export_csv(
    State(state): State<PetsState>,
    Path(pet_id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let mut history = state
        .pets
        .weight_history(&pet_id)
        .await
        .map_err(ApiError::internal)?;
    if let Some(days_param) = query.days.as_deref().filter(|d| !d.is_empty()) {
        let days = string_to_number(days_param);
        if !days.is_nan() && days > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            let cutoff = (state.clock.now_ms() as f64 - days * 86_400_000.0).trunc();
            #[allow(clippy::cast_precision_loss)]
            let cutoff_valid = cutoff.abs() <= MAX_DATE_MS as f64;
            history.retain(|row| {
                // `new Date(timestamp) >= cutoff`: an invalid date on either side is false.
                cutoff_valid
                    && date_parse(&row.timestamp, &state.tz).is_some_and(|at| {
                        #[allow(clippy::cast_precision_loss)]
                        let at = at as f64;
                        at >= cutoff
                    })
            });
        }
    }
    let mut lines = vec!["timestamp,weight_lbs".to_owned()];
    for row in &history {
        lines.push(format!(
            "{},{}",
            row.timestamp,
            number_to_string(row.weight)
        ));
    }
    let pet = state
        .pets
        .get_pet(&pet_id)
        .await
        .map_err(ApiError::internal)?;
    let filename = match pet {
        Some(pet) => format!("{}-weight.csv", pet.name.to_lowercase()),
        None => format!("{pet_id}-weight.csv"),
    };
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(ApiError::internal)?;
    let mut response = lines.join("\n").into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv"));
    headers.insert(header::CONTENT_DISPOSITION, disposition);
    Ok(response)
}
