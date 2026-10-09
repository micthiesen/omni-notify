//! Streamer routes (`server.ts`): `GET /api/streamers`,
//! `GET /api/trigger-channels`, `GET /api/streamers/:id/metrics`,
//! `GET /api/streamers/:id/sessions`.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use omni_api::streamers::{
    StreamSessionsResponse, StreamerMetricsResponse, StreamersResponse, TriggerChannelsResponse,
};
use omni_server_kit::{ApiError, ApiResult};
use omni_store::Store;

use crate::display::{display_views, metrics_view, session_view};
use crate::error::LiveError;
use crate::metrics::{get_platform_viewer_metrics, get_viewer_metrics};
use crate::sessions::get_sessions;
use crate::streamers::Roster;
use crate::trigger_channels::to_trigger_channels;

#[derive(Clone)]
pub struct LiveRoutesState {
    pub store: Store,
    pub roster: Roster,
}

/// Logged once (at ERROR) by `ApiError`, rendered as Hono's opaque 500.
fn internal(error: LiveError) -> ApiError {
    ApiError::internal(error)
}

fn unknown_streamer() -> ApiError {
    ApiError::not_found("Unknown streamer")
}

async fn streamers(State(state): State<LiveRoutesState>) -> ApiResult<StreamersResponse> {
    let streamers = display_views(&state.store, &state.roster.snapshot())
        .await
        .map_err(internal)?;
    Ok(Json(StreamersResponse { streamers }))
}

async fn trigger_channels(State(state): State<LiveRoutesState>) -> Json<TriggerChannelsResponse> {
    Json(TriggerChannelsResponse {
        channels: to_trigger_channels(&state.roster.snapshot()),
    })
}

async fn metrics(
    State(state): State<LiveRoutesState>,
    Path(id): Path<String>,
) -> ApiResult<StreamerMetricsResponse> {
    if !state.roster.contains(&id) {
        return Err(unknown_streamer());
    }
    let aggregate = get_viewer_metrics(&state.store, &id)
        .await
        .map_err(internal)?;
    let platforms = get_platform_viewer_metrics(&state.store, &id)
        .await
        .map_err(internal)?;
    Ok(Json(metrics_view(&aggregate, &platforms)))
}

async fn sessions(
    State(state): State<LiveRoutesState>,
    Path(id): Path<String>,
) -> ApiResult<StreamSessionsResponse> {
    if !state.roster.contains(&id) {
        return Err(unknown_streamer());
    }
    let data = get_sessions(&state.store, &id).await.map_err(internal)?;
    Ok(Json(StreamSessionsResponse {
        sessions: data.sessions.iter().rev().map(session_view).collect(),
    }))
}

/// The streamer routes with state applied.
pub fn router(state: LiveRoutesState) -> Router {
    Router::new()
        .route("/api/streamers", get(streamers))
        .route("/api/trigger-channels", get(trigger_channels))
        .route("/api/streamers/{id}/metrics", get(metrics))
        .route("/api/streamers/{id}/sessions", get(sessions))
        .with_state(state)
}
