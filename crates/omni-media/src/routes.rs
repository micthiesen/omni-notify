//! `/api/recommendations*` routes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use omni_api::media::{
    MediaType, OnDeckItem, Recommendation, RecommendationFeedback, RecommendationLinks,
    RecommendationResponse, RecommendationStatus, RecommendationsResponse, RunResponse,
    ShortlistScores, TasteProfileResponse, WatchlistResult,
};
use omni_core::js::{encode_uri_component, trim};
use omni_server_kit::{ApiError, JsonBody, api_error};
use omni_tasks::{RunNowError, TaskRegistry};
use serde_json::Value;

use crate::error::max_recommendations_message;
use crate::persistence::{
    FeedbackInput, RecommendationData, get_all_recommendations, get_recommendation,
    set_recommendation_feedback,
};
use crate::pipeline::{MAX_RECOMMENDATIONS_PER_RUN, validate_max_recommendations};
use crate::services::MediaServices;
use crate::task::TASK_NAME;
use crate::taste::get_latest_taste_profile;

const LOG: &str = "Server";

#[derive(Clone)]
pub struct RouteState {
    pub services: Arc<MediaServices>,
    pub tasks: TaskRegistry,
}

pub fn router(state: RouteState) -> Router {
    Router::new()
        .route("/api/recommendations", get(list))
        .route("/api/recommendations/taste-profile", get(taste_profile))
        // `GET /run` is treated as `GET /:id` and answers 404, not 405.
        .route("/api/recommendations/run", post(run).get(get_run_as_id))
        .route("/api/recommendations/{id}", get(get_one))
        .route("/api/recommendations/{id}/feedback", post(feedback))
        .with_state(state)
}

/// Radarr's movie page slug is the TMDB id once the movie is tracked;
/// otherwise its add-new search. Sonarr needs its own slug (captured at add
/// time); older rows fall back to the add-new search by title.
pub fn build_manager_link(rec: &RecommendationData) -> String {
    match rec.media_type {
        MediaType::Movie => {
            let in_radarr = matches!(
                rec.watchlist_result,
                Some(WatchlistResult::Added | WatchlistResult::AlreadyExists)
            );
            if in_radarr {
                format!("http://radarr.boris/movie/{}", rec.tmdb_id)
            } else {
                format!(
                    "http://radarr.boris/add/new?term={}",
                    encode_uri_component(&format!("tmdb:{}", rec.tmdb_id))
                )
            }
        }
        MediaType::Tv => match rec.manager_slug.as_deref().filter(|s| !s.is_empty()) {
            Some(slug) => format!("http://sonarr.boris/series/{slug}"),
            None => format!(
                "http://sonarr.boris/add/new?term={}",
                encode_uri_component(&rec.title)
            ),
        },
    }
}

#[allow(clippy::cast_precision_loss)]
fn num(value: i64) -> f64 {
    value as f64
}

/// A recommendation row as the API serves it.
pub fn serialize_recommendation(rec: &RecommendationData) -> Recommendation {
    Recommendation {
        recommendation_id: rec.recommendation_id.clone(),
        canonical_id: rec.canonical_id.clone(),
        tmdb_id: num(rec.tmdb_id),
        media_type: rec.media_type,
        title: rec.title.clone(),
        year: rec.year.map(num),
        poster_path: rec.poster_path.clone(),
        status: rec.status(),
        why_for_user: rec.why_for_user.clone(),
        caveats: rec.caveats.clone().unwrap_or_default(),
        run_date: rec.run_date.clone(),
        recommended_at: num(rec.recommended_at),
        notified_at: rec.notified_at.map(num),
        started_at: rec.started_at.map(num),
        resolved_at: rec.resolved_at.map(num),
        watchlist_result: rec.watchlist_result,
        confidence: rec.confidence,
        feedback: rec.feedback,
        feedback_at: rec.feedback_at.map(num),
        feedback_note: rec.feedback_note.clone(),
        source: rec.source.map(|s| s.as_str().to_owned()),
        genres: rec.genres.clone().unwrap_or_default(),
        runtime_minutes: rec.runtime_minutes,
        season_count: rec.season_count,
        episode_count: rec.episode_count,
        series_status: rec.series_status.clone(),
        original_language: rec.original_language.clone(),
        origin_countries: rec.origin_countries.clone().unwrap_or_default(),
        creators: rec.creators.clone().unwrap_or_default(),
        cast: rec.cast.clone().unwrap_or_default(),
        keywords: rec.keywords.clone().unwrap_or_default(),
        certification: rec.certification.clone(),
        shortlist_scores: rec.shortlist_scores.as_ref().map(|s| ShortlistScores {
            taste_match: s.taste_match,
            novelty: s.novelty,
            effort_fit: s.effort_fit,
            composite: s.composite,
            risks: s.risks.clone(),
        }),
        links: RecommendationLinks {
            tmdb: crate::tmdb::tmdb_url(rec.media_type, rec.tmdb_id),
            plex: format!(
                "http://plex.boris/web/index.html#!/search?pivot=top&query={}",
                encode_uri_component(&rec.title)
            ),
            manager: build_manager_link(rec),
        },
    }
}

/// An On Deck item.
pub fn serialize_on_deck(rec: &RecommendationData) -> OnDeckItem {
    OnDeckItem {
        recommendation_id: rec.recommendation_id.clone(),
        title: rec.title.clone(),
        media_type: rec.media_type,
        year: rec.year.map(num),
        poster_path: rec.poster_path.clone(),
        why_for_user: rec.why_for_user.clone(),
        recommended_at: num(rec.recommended_at),
    }
}

async fn list(State(state): State<RouteState>) -> Result<Json<RecommendationsResponse>, ApiError> {
    let all = get_all_recommendations(&state.services.store)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(RecommendationsResponse {
        recommendations: all.iter().map(serialize_recommendation).collect(),
    }))
}

async fn taste_profile(
    State(state): State<RouteState>,
) -> Result<Json<TasteProfileResponse>, ApiError> {
    let latest = get_latest_taste_profile(&state.services.store)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(TasteProfileResponse {
        profile: latest.map(|p| p.profile),
    }))
}

async fn get_one(
    State(state): State<RouteState>,
    Path(id): Path<String>,
) -> Result<Json<RecommendationResponse>, ApiError> {
    lookup(&state, &id).await
}

async fn get_run_as_id(
    State(state): State<RouteState>,
) -> Result<Json<RecommendationResponse>, ApiError> {
    lookup(&state, "run").await
}

async fn lookup(state: &RouteState, id: &str) -> Result<Json<RecommendationResponse>, ApiError> {
    match get_recommendation(&state.services.store, id)
        .await
        .map_err(ApiError::internal)?
    {
        Some(rec) => Ok(Json(RecommendationResponse {
            recommendation: serialize_recommendation(&rec),
        })),
        None => Err(ApiError::not_found("Recommendation not found")),
    }
}

/// The feedback body: optional rating literal, optional note of at most
/// 1000 UTF-16 units; anything else is invalid.
pub fn parse_feedback_body(body: &Value) -> Option<FeedbackInput> {
    let object = body.as_object()?;
    let feedback = match object.get("feedback") {
        None => None,
        Some(value) => Some(serde_json::from_value::<RecommendationFeedback>(value.clone()).ok()?),
    };
    let note = match object.get("note") {
        None => None,
        Some(Value::String(note)) if omni_core::js::utf16_len(note) <= 1000 => Some(note.clone()),
        Some(_) => return None,
    };
    Some(FeedbackInput { feedback, note })
}

async fn feedback(
    State(state): State<RouteState>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<Value>,
) -> Result<Json<RecommendationResponse>, ApiError> {
    let Some(input) = parse_feedback_body(&body) else {
        return Err(ApiError::bad_request("Invalid recommendation feedback"));
    };
    let note = input
        .note
        .as_deref()
        .map(trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    if input.feedback.is_none() && note.is_none() {
        return Err(ApiError::bad_request("A rating or a note is required"));
    }
    let services = &state.services;
    let Some(existing) = get_recommendation(&services.store, &id)
        .await
        .map_err(ApiError::internal)?
    else {
        return Err(ApiError::not_found("Recommendation not found"));
    };
    if matches!(
        existing.status(),
        RecommendationStatus::Pending | RecommendationStatus::Failed
    ) {
        return Err(ApiError::conflict(
            "Undelivered recommendations cannot be rated",
        ));
    }
    let updated = set_recommendation_feedback(
        &services.store,
        services.now(),
        &id,
        FeedbackInput {
            feedback: input.feedback,
            note,
        },
    )
    .await
    .map_err(ApiError::internal)?;
    match updated {
        Some(rec) => Ok(Json(RecommendationResponse {
            recommendation: serialize_recommendation(&rec),
        })),
        None => Err(ApiError::not_found("Recommendation not found")),
    }
}

async fn run(State(state): State<RouteState>, JsonBody(body): JsonBody<Value>) -> Response {
    let requested = body
        .as_object()
        .and_then(|o| o.get("maxRecommendations"))
        .and_then(Value::as_f64)
        .filter(|v| validate_max_recommendations(*v).is_ok());
    let Some(max) = requested else {
        return api_error(
            StatusCode::BAD_REQUEST,
            max_recommendations_message(MAX_RECOMMENDATIONS_PER_RUN),
        );
    };
    let input = serde_json::json!({ "maxRecommendations": max_value(max) });
    match state.tasks.run_now(TASK_NAME, Some(input)) {
        Ok(run_id) => {
            tracing::info!(
                target: LOG,
                "Manual recommendation run requested for up to {} item(s)",
                omni_core::js::number_to_string(max)
            );
            (StatusCode::ACCEPTED, Json(RunResponse { run_id })).into_response()
        }
        Err(error) => run_now_error(error),
    }
}

/// `maxRecommendations` as an integer JSON number.
fn max_value(max: f64) -> Value {
    #[allow(clippy::cast_possible_truncation)]
    Value::from(max as i64)
}

/// The error response for a refused manual run.
pub fn run_now_error(error: RunNowError) -> Response {
    match error {
        RunNowError::NotFound { .. } => api_error(StatusCode::NOT_FOUND, error.to_string()),
        RunNowError::AlreadyRunning { .. } => api_error(StatusCode::CONFLICT, error.to_string()),
        RunNowError::ManualInputUnsupported { .. } => {
            api_error(StatusCode::BAD_REQUEST, error.to_string())
        }
        other => ApiError::internal(other).into_response(),
    }
}
