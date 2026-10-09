//! Parcel delivery status: one scheduled, budgeted read of Parcel's
//! deliveries endpoint into a durable cache, served by `GET /api/parcels`
//! and the `parcels_list` / `parcels_get` MCP tools. Readers only ever see
//! the cache; nothing outside [`task`] calls Parcel, and the add path
//! (`parcel_api`) is separate.

pub mod api;
pub mod state;
pub mod task;
pub mod view;

use axum::Json;
use axum::extract::State;
use axum::routing::get;
use omni_api::parcels::{PARCELS, ParcelsResponse};
use omni_server_kit::ApiError;
use omni_store::Store;

/// The cached view.
pub async fn load_response(
    store: &Store,
    configured: bool,
) -> Result<ParcelsResponse, omni_store::StoreError> {
    let (snapshot, read_state, submissions) = state::load(store).await?;
    Ok(view::response(
        configured,
        snapshot.as_ref(),
        read_state.as_ref(),
        &submissions,
    ))
}

#[derive(Clone)]
struct RouteState {
    store: Store,
    configured: bool,
}

/// `GET /api/parcels`.
pub fn router(store: Store, configured: bool) -> axum::Router {
    axum::Router::new()
        .route(PARCELS, get(list))
        .with_state(RouteState { store, configured })
}

async fn list(State(state): State<RouteState>) -> Result<Json<ParcelsResponse>, ApiError> {
    load_response(&state.store, state.configured)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}
