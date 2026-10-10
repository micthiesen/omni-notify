//! Streamer configuration routes (the UI's editor):
//! `GET /api/streamer-config`, `POST /api/streamer-config/streamers`,
//! `PATCH|DELETE /api/streamer-config/streamers/:id`,
//! `PUT /api/streamer-config/order` and `PUT /api/streamer-config/settings`.
//! Pushover tokens can be set here but are never returned.

use axum::extract::{Path, State};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use omni_api::streamer_config::{
    LiveSettings, StreamerConfigCreate, StreamerConfigOrder, StreamerConfigPatch,
    StreamerConfigResponse, StreamerConfigView,
};
use omni_server_kit::{ApiError, ApiResult, JsonBody};

use crate::config::{ConfigError, StreamerConfigService};

fn api_error(error: ConfigError) -> ApiError {
    match error {
        ConfigError::Invalid(message) => ApiError::bad_request(message),
        error @ ConfigError::NotFound(_) => ApiError::not_found(error.to_string()),
        error => ApiError::internal(error),
    }
}

async fn list(State(service): State<StreamerConfigService>) -> ApiResult<StreamerConfigResponse> {
    service.list().await.map(Json).map_err(api_error)
}

async fn create(
    State(service): State<StreamerConfigService>,
    JsonBody(body): JsonBody<StreamerConfigCreate>,
) -> ApiResult<StreamerConfigView> {
    service.create(body).await.map(Json).map_err(api_error)
}

async fn update(
    State(service): State<StreamerConfigService>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<StreamerConfigPatch>,
) -> ApiResult<StreamerConfigView> {
    service.update(&id, body).await.map(Json).map_err(api_error)
}

async fn remove(
    State(service): State<StreamerConfigService>,
    Path(id): Path<String>,
) -> ApiResult<StreamerConfigView> {
    service.delete(&id).await.map(Json).map_err(api_error)
}

async fn reorder(
    State(service): State<StreamerConfigService>,
    JsonBody(body): JsonBody<StreamerConfigOrder>,
) -> ApiResult<StreamerConfigResponse> {
    service.reorder(body.ids).await.map_err(api_error)?;
    service.list().await.map(Json).map_err(api_error)
}

async fn settings(
    State(service): State<StreamerConfigService>,
    JsonBody(body): JsonBody<LiveSettings>,
) -> ApiResult<LiveSettings> {
    service
        .update_settings(body)
        .await
        .map(Json)
        .map_err(api_error)
}

/// The configuration routes with state applied.
pub fn router(service: StreamerConfigService) -> Router {
    Router::new()
        .route("/api/streamer-config", get(list))
        .route("/api/streamer-config/streamers", post(create))
        .route(
            "/api/streamer-config/streamers/{id}",
            patch(update).delete(remove),
        )
        .route("/api/streamer-config/order", put(reorder))
        .route("/api/streamer-config/settings", put(settings))
        .with_state(service)
}
