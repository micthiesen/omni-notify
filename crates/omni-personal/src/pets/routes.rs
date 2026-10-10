//! `GET /api/pets`, `GET /api/pets/health`, `GET /api/pets/:petId/export.csv`,
//! and `POST /api/pets/health/dismiss` and `/restore` for health findings.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use jiff::tz::TimeZone;
use omni_api::pets::{
    DailyVisit, Pet, PetHealthDismissRequest, PetHealthDismissResponse, WeightEntry,
};
use omni_core::clock::SharedClock;
use omni_core::js::{
    MAX_DATE_MS, date_parse, json_stringify, math_round, number_to_string, string_to_number,
    to_iso_string,
};
use omni_server_kit::{ApiError, JsonBody, json_response};
use serde::Deserialize;

use super::alerts::HealthLedger;
use super::dismissals::dismissable;
use super::health;
use super::persistence::PetStore;

#[derive(Clone)]
struct PetsState {
    pets: PetStore,
    ledger: HealthLedger,
    clock: SharedClock,
    tz: TimeZone,
}

/// Weekly blocks per card unless `?weeks=` asks for 1 to 52.
pub const DEFAULT_HEALTH_WEEKS: u32 = 26;
pub const MAX_HEALTH_WEEKS: u32 = 52;

/// The health response shared by the route and the `pets_read` `trend` resource.
pub async fn health_response(
    pets: &PetStore,
    ledger: &HealthLedger,
    now: i64,
    tz: &TimeZone,
    weeks: u32,
) -> Result<omni_api::pets::PetHealthResponse, omni_store::StoreError> {
    let all = pets.all_pets_with_history().await?;
    let evaluation = health::evaluate(&all, now, tz, weeks);
    let alerts = ledger.all().await?.iter().map(|row| row.info()).collect();
    let dismissed = ledger.dismissed().await?;
    let mut response = health::response(&evaluation, alerts, now);
    for trend in &mut response.pets {
        for finding in &mut trend.findings {
            finding.dismissed_at = dismissed
                .get(&(trend.pet_id.clone(), finding.kind))
                .map(|&at| to_iso_string(at));
        }
    }
    Ok(response)
}

/// `Math.round(n * 100) / 100`.
fn round2(n: f64) -> f64 {
    math_round(n * 100.0) / 100.0
}

/// The pet routes, state applied.
pub fn router(pets: PetStore, ledger: HealthLedger, clock: SharedClock, tz: TimeZone) -> Router {
    Router::new()
        .route("/api/pets", get(list_pets))
        .route("/api/pets/health", get(pet_health))
        .route("/api/pets/health/dismiss", post(dismiss_finding))
        .route("/api/pets/health/restore", post(restore_finding))
        .route("/api/pets/{pet_id}/export.csv", get(export_csv))
        .with_state(PetsState {
            pets,
            ledger,
            clock,
            tz,
        })
}

#[derive(Deserialize)]
struct HealthQuery {
    weeks: Option<String>,
}

async fn pet_health(
    State(state): State<PetsState>,
    Query(query): Query<HealthQuery>,
) -> Result<Response, ApiError> {
    let weeks = match query.weeks.as_deref().filter(|w| !w.is_empty()) {
        None => DEFAULT_HEALTH_WEEKS,
        Some(text) => text
            .parse::<u32>()
            .ok()
            .filter(|w| (1..=MAX_HEALTH_WEEKS).contains(w))
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "weeks must be an integer from 1 to {MAX_HEALTH_WEEKS}"
                ))
            })?,
    };
    let response = health_response(
        &state.pets,
        &state.ledger,
        state.clock.now_ms(),
        &state.tz,
        weeks,
    )
    .await
    .map_err(ApiError::internal)?;
    let value = serde_json::to_value(&response).map_err(ApiError::internal)?;
    Ok(json_response(StatusCode::OK, json_stringify(&value)))
}

/// Dismisses a currently tripped per-pet finding for its episode.
async fn dismiss_finding(
    State(state): State<PetsState>,
    JsonBody(request): JsonBody<PetHealthDismissRequest>,
) -> Result<Response, ApiError> {
    if !dismissable(request.kind) {
        return Err(ApiError::bad_request(format!(
            "{} cannot be dismissed; it clears when readings resume",
            request.kind.as_str()
        )));
    }
    let now = state.clock.now_ms();
    let all = state
        .pets
        .all_pets_with_history()
        .await
        .map_err(ApiError::internal)?;
    let evaluation = health::evaluate(&all, now, &state.tz, 1);
    let Some(trend) = evaluation
        .trends
        .iter()
        .find(|t| t.pet_id == request.pet_id)
    else {
        return Err(ApiError::not_found("Pet not found"));
    };
    if !trend.findings.iter().any(|f| f.kind == request.kind) {
        return Err(ApiError::conflict(format!(
            "{} is not tripped for {}",
            request.kind.as_str(),
            trend.name
        )));
    }
    state
        .ledger
        .dismiss(&request.pet_id, request.kind, now)
        .await
        .map_err(ApiError::internal)?;
    dismiss_response(request, Some(to_iso_string(now)))
}

/// Shows a dismissed finding again (idempotent).
async fn restore_finding(
    State(state): State<PetsState>,
    JsonBody(request): JsonBody<PetHealthDismissRequest>,
) -> Result<Response, ApiError> {
    state
        .ledger
        .undismiss(&request.pet_id, request.kind)
        .await
        .map_err(ApiError::internal)?;
    dismiss_response(request, None)
}

fn dismiss_response(
    request: PetHealthDismissRequest,
    dismissed_at: Option<String>,
) -> Result<Response, ApiError> {
    let body = PetHealthDismissResponse {
        pet_id: request.pet_id,
        kind: request.kind,
        dismissed_at,
    };
    let value = serde_json::to_value(&body).map_err(ApiError::internal)?;
    Ok(json_response(StatusCode::OK, json_stringify(&value)))
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
