//! `/api/podcast-recommendations` routes (`src/server.ts` 279-308, 1315-1392).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use omni_api::podcasts as api;
use omni_core::clock::SharedClock;
use omni_server_kit::{ApiError, ApiResult, JsonBody};
use omni_store::Store;
use omni_tasks::{RunNowError, TaskRegistry};
use serde_json::Value;

use crate::persistence::{
    PodcastFeedback, PodcastQueueResult, PodcastRecommendationData, PodcastRecommendationStatus,
    get_all_podcast_recommendations, get_podcast_recommendation,
    set_podcast_recommendation_feedback,
};
use crate::pipeline::{MAX_PODCAST_RECOMMENDATIONS_PER_RUN, range_error};
use crate::reflection::store::get_latest_podcast_taste_profile;
use crate::reflection::types::{PodcastTasteClaim, PodcastTasteProfileData};
use crate::task::TASK_NAME;

const LOG: &str = "Server";
const NOTE_MAX_UTF16: usize = 1000;

#[derive(Clone)]
pub struct RoutesState {
    pub store: Store,
    pub clock: SharedClock,
    pub tasks: TaskRegistry,
}

pub fn router(state: RoutesState) -> Router {
    Router::new()
        .route("/api/podcast-recommendations", get(list))
        // Static path registered alongside `/{id}`; axum gives it precedence.
        .route(
            "/api/podcast-recommendations/taste-profile",
            get(taste_profile),
        )
        .route("/api/podcast-recommendations/run", post(run))
        .route("/api/podcast-recommendations/{id}", get(detail))
        .route("/api/podcast-recommendations/{id}/feedback", post(feedback))
        .with_state(state)
}

fn status_dto(status: PodcastRecommendationStatus) -> api::PodcastRecommendationStatus {
    match status {
        PodcastRecommendationStatus::Pending => api::PodcastRecommendationStatus::Pending,
        PodcastRecommendationStatus::Notified => api::PodcastRecommendationStatus::Notified,
        PodcastRecommendationStatus::Listened => api::PodcastRecommendationStatus::Listened,
        PodcastRecommendationStatus::Abandoned => api::PodcastRecommendationStatus::Abandoned,
        PodcastRecommendationStatus::Ignored => api::PodcastRecommendationStatus::Ignored,
        PodcastRecommendationStatus::Failed => api::PodcastRecommendationStatus::Failed,
    }
}

pub fn feedback_dto(feedback: PodcastFeedback) -> api::PodcastFeedback {
    match feedback {
        PodcastFeedback::GoodPick => api::PodcastFeedback::GoodPick,
        PodcastFeedback::NotForMe => api::PodcastFeedback::NotForMe,
    }
}

fn queue_dto(result: PodcastQueueResult) -> api::PodcastQueueResult {
    match result {
        PodcastQueueResult::Queued => api::PodcastQueueResult::Queued,
        PodcastQueueResult::AlreadyQueued => api::PodcastQueueResult::AlreadyQueued,
        PodcastQueueResult::NotQueued => api::PodcastQueueResult::NotQueued,
    }
}

/// `serializePodcastRecommendation` (server.ts).
pub fn serialize_recommendation(rec: &PodcastRecommendationData) -> api::PodcastRecommendation {
    api::PodcastRecommendation {
        recommendation_id: rec.recommendation_id.clone(),
        show_title: rec.show_title.clone(),
        episode_title: rec.episode_title.clone(),
        feed_url: rec.feed_url.clone(),
        itunes_id: rec.itunes_id,
        artwork_url: rec.artwork_url.clone(),
        episode_url: rec.episode_url.clone(),
        published_at: rec.published_at,
        duration_minutes: rec.duration_minutes,
        status: status_dto(rec.status),
        why_for_user: rec.why_for_user.clone(),
        caveats: rec.caveats.clone().unwrap_or_default(),
        confidence: rec.confidence,
        shortlist_scores: rec
            .shortlist_scores
            .as_ref()
            .map(|s| api::PodcastShortlistScores {
                taste_match: s.taste_match,
                novelty: s.novelty,
                composite: s.composite,
                risks: s.risks.clone(),
            }),
        discovered_via: rec.discovered_via.clone(),
        source_url: rec.source_url.clone(),
        matched_voices: rec.matched_voices.clone().unwrap_or_default(),
        recommended_at: rec.recommended_at,
        notified_at: rec.notified_at,
        queue_result: rec.queue_result.map(queue_dto),
        feedback: rec.feedback.map(feedback_dto),
        feedback_at: rec.feedback_at,
        feedback_note: rec.feedback_note.clone(),
    }
}

fn claims_dto(claims: &[PodcastTasteClaim]) -> Vec<api::PodcastTasteClaim> {
    claims
        .iter()
        .map(|c| api::PodcastTasteClaim {
            claim: c.claim.clone(),
            confidence: c.confidence,
            evidence_ids: c.evidence_ids.clone(),
        })
        .collect()
}

pub fn serialize_profile(profile: &PodcastTasteProfileData) -> api::PodcastTasteProfile {
    let stats = &profile.stats;
    api::PodcastTasteProfile {
        summary: profile.summary.clone(),
        stable_preferences: claims_dto(&profile.stable_preferences),
        conditional_preferences: claims_dto(&profile.conditional_preferences),
        aversions: claims_dto(&profile.aversions),
        current_saturation: claims_dto(&profile.current_saturation),
        exploration_targets: claims_dto(&profile.exploration_targets),
        uncertainties: claims_dto(&profile.uncertainties),
        profile_id: profile.profile_id.clone(),
        version: profile.version,
        generated_at: profile.generated_at,
        evidence_fingerprint: profile.evidence_fingerprint.clone(),
        evidence_count: profile.evidence_count,
        model_id: profile.model_id.clone(),
        prompt_version: profile.prompt_version.clone(),
        stats: api::PodcastTasteStats {
            listened_episodes: stats.listened_episodes,
            started_episodes: stats.started_episodes,
            starred_episodes: stats.starred_episodes,
            distinct_shows: stats.distinct_shows,
            recommendations: api::PodcastRecommendationCounts {
                total: stats.recommendations.total,
                listened: stats.recommendations.listened,
                abandoned: stats.recommendations.abandoned,
                ignored: stats.recommendations.ignored,
                failed: stats.recommendations.failed,
                awaiting_outcome: stats.recommendations.awaiting_outcome,
            },
            feedback: api::PodcastFeedbackCounts {
                good_pick: stats.feedback.good_pick,
                not_for_me: stats.feedback.not_for_me,
            },
        },
    }
}

async fn list(State(state): State<RoutesState>) -> ApiResult<api::PodcastRecommendationsResponse> {
    let records = get_all_podcast_recommendations(&state.store)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(api::PodcastRecommendationsResponse {
        recommendations: records.iter().map(serialize_recommendation).collect(),
    }))
}

/// TS handed `c.json` the un-run Effect here; this returns the profile the
/// frontend's schema expects (documented fix).
async fn taste_profile(
    State(state): State<RoutesState>,
) -> ApiResult<api::PodcastTasteProfileResponse> {
    let profile = get_latest_podcast_taste_profile(&state.store)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(api::PodcastTasteProfileResponse {
        profile: profile.as_ref().map(serialize_profile),
    }))
}

async fn detail(
    State(state): State<RoutesState>,
    Path(id): Path<String>,
) -> ApiResult<api::PodcastRecommendationResponse> {
    match get_podcast_recommendation(&state.store, &id)
        .await
        .map_err(ApiError::internal)?
    {
        Some(rec) => Ok(Json(api::PodcastRecommendationResponse {
            recommendation: serialize_recommendation(&rec),
        })),
        None => Err(ApiError::not_found("Recommendation not found")),
    }
}

/// The feedback body after schema validation.
#[derive(Debug, PartialEq, Eq)]
pub struct FeedbackBody {
    pub feedback: Option<PodcastFeedback>,
    pub note: Option<String>,
}

/// Effect Schema `{feedback?: "good_pick"|"not_for_me", note?: string(max 1000)}`.
pub fn parse_feedback_body(body: &Value) -> Option<FeedbackBody> {
    let object = body.as_object()?;
    let feedback = match object.get("feedback") {
        None => None,
        Some(Value::String(s)) if s == "good_pick" => Some(PodcastFeedback::GoodPick),
        Some(Value::String(s)) if s == "not_for_me" => Some(PodcastFeedback::NotForMe),
        Some(_) => return None,
    };
    let note = match object.get("note") {
        None => None,
        Some(Value::String(s)) if omni_core::js::utf16_len(s) <= NOTE_MAX_UTF16 => Some(s.clone()),
        Some(_) => return None,
    };
    Some(FeedbackBody { feedback, note })
}

async fn feedback(
    State(state): State<RoutesState>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<Value>,
) -> ApiResult<api::PodcastRecommendationResponse> {
    let Some(parsed) = parse_feedback_body(&body) else {
        return Err(ApiError::bad_request("Invalid recommendation feedback"));
    };
    let note = parsed
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    if parsed.feedback.is_none() && note.is_none() {
        return Err(ApiError::bad_request("A rating or a note is required"));
    }
    let Some(existing) = get_podcast_recommendation(&state.store, &id)
        .await
        .map_err(ApiError::internal)?
    else {
        return Err(ApiError::not_found("Recommendation not found"));
    };
    if matches!(
        existing.status,
        PodcastRecommendationStatus::Pending | PodcastRecommendationStatus::Failed
    ) {
        return Err(ApiError::conflict(
            "Undelivered recommendations cannot be rated",
        ));
    }
    let updated = set_podcast_recommendation_feedback(
        &state.store,
        &id,
        parsed.feedback,
        note,
        state.clock.now_ms(),
    )
    .await
    .map_err(ApiError::internal)?;
    match updated {
        Some(rec) => Ok(Json(api::PodcastRecommendationResponse {
            recommendation: serialize_recommendation(&rec),
        })),
        None => Err(ApiError::not_found("Recommendation not found")),
    }
}

/// `{maxRecommendations}` as an integer 1..=5.
pub fn parse_run_body(body: &Value) -> Option<i64> {
    let value = body.as_object()?.get("maxRecommendations")?.as_f64()?;
    #[allow(clippy::cast_precision_loss)]
    let max = MAX_PODCAST_RECOMMENDATIONS_PER_RUN as f64;
    (value.fract() == 0.0 && (1.0..=max).contains(&value)).then_some(value as i64)
}

async fn run(
    State(state): State<RoutesState>,
    JsonBody(body): JsonBody<Value>,
) -> Result<Response, ApiError> {
    let Some(max) = parse_run_body(&body) else {
        return Err(ApiError::bad_request(range_error()));
    };
    match state.tasks.run_now(
        TASK_NAME,
        Some(serde_json::json!({ "maxRecommendations": max })),
    ) {
        Ok(run_id) => {
            tracing::info!(
                target: LOG,
                "Manual podcast recommendation run requested for up to {max} episode(s)"
            );
            Ok((
                StatusCode::ACCEPTED,
                Json(omni_api::tasks::RunNowResponse { run_id }),
            )
                .into_response())
        }
        Err(error @ RunNowError::NotFound { .. }) => Err(ApiError::not_found(error.to_string())),
        Err(error @ RunNowError::AlreadyRunning { .. }) => {
            Err(ApiError::conflict(error.to_string()))
        }
        Err(error @ RunNowError::ManualInputUnsupported { .. }) => {
            Err(ApiError::bad_request(error.to_string()))
        }
        Err(other) => Err(ApiError::internal(other)),
    }
}
