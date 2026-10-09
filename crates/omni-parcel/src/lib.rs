//! Parcel tracker email pipeline (WP02): candidate filter, tracking-number
//! extraction, carrier validation against the live Parcel list, durable
//! dedup with reservation before submission, and the delivery-forget route.

pub mod carriers;
pub mod error;
pub mod extraction;
pub mod filter;
pub mod log_file;
pub mod parcel_api;
pub mod persistence;
pub mod pipeline;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::routing::delete;
use omni_api::email::DeletedResponse;
use omni_email::triage::EmailTriage;
use omni_http::public::PublicHttpClient;
use omni_runtime::{AppContext, ManagedEntity, Subsystem};
use omni_server_kit::ApiError;
use omni_store::Store;
use omni_store::entity::EntityDescriptor;

use crate::carriers::carrier_map::CarrierDirectory;
use crate::extraction::ModelExtractor;
use crate::parcel_api::ParcelApi;
use crate::persistence::SubmittedDelivery;
use crate::pipeline::{DeliveryPipeline, PipelineDeps};

const LOG: &str = "Main:ParcelTracker";
const SERVER_LOG: &str = "Main:Server";

#[derive(Debug, thiserror::Error)]
pub enum ParcelSetupError {
    #[error(transparent)]
    Http(#[from] omni_http::HttpError),
}

pub fn entities() -> Vec<EntityDescriptor> {
    vec![EntityDescriptor::of::<SubmittedDelivery>()]
}

pub fn managed_entities() -> Vec<ManagedEntity> {
    let entity = EntityDescriptor::of::<SubmittedDelivery>();
    vec![ManagedEntity {
        slug: entity.name,
        label: "Submitted deliveries",
        description: "Parcel tracking numbers already sent to Parcel.app.",
        warning: Some("This is a deduplication gate. Deleted rows may be submitted again."),
        entity,
        primary_key: &["trackingNumber"],
        can_delete: None,
        after_delete: None,
    }]
}

/// `DELETE /api/parcel-tracker/deliveries/:trackingNumber`: forget a submitted
/// tracking number so a future email can resubmit it.
pub fn router(store: Store) -> axum::Router {
    axum::Router::new()
        .route(
            "/api/parcel-tracker/deliveries/{tracking_number}",
            delete(forget_delivery),
        )
        .with_state(store)
}

async fn forget_delivery(
    State(store): State<Store>,
    Path(tracking_number): Path<String>,
) -> Result<Json<DeletedResponse>, ApiError> {
    let deleted = persistence::forget(&store, &tracking_number)
        .await
        .map_err(ApiError::internal)?;
    if !deleted {
        return Err(ApiError::not_found("Unknown tracking number"));
    }
    tracing::info!(target: SERVER_LOG, "Forgot submitted delivery {tracking_number}");
    Ok(Json(DeletedResponse { deleted: true }))
}

/// `createParcelHandler`: `None` (logged) without `PARCEL_API_KEY`.
pub fn handler(
    ctx: &AppContext,
    triage: EmailTriage,
) -> Result<Option<Arc<DeliveryPipeline>>, ParcelSetupError> {
    let Some(api_key) = ctx
        .config
        .parcel_api_key
        .clone()
        .filter(|key| !key.is_empty())
    else {
        tracing::info!(target: LOG, "Disabled: missing PARCEL_API_KEY");
        return Ok(None);
    };
    let carriers = Arc::new(CarrierDirectory::new(
        PublicHttpClient::new(&ctx.http),
        ctx.clock.clone(),
    )?);
    let deps = PipelineDeps {
        store: ctx.store.clone(),
        run_logs: ctx.run_logs(),
        triage,
        extractor: Arc::new(ModelExtractor::new(
            ctx.ai.clone(),
            ctx.config.clone(),
            carriers.clone(),
        )),
        submitter: Arc::new(ParcelApi::new(
            ctx.http.clone(),
            api_key,
            ctx.side_effects,
            ctx.clock.clone(),
        )?),
        carriers,
        self_address: ctx.config.email_self_address().map(str::to_owned),
        logs_path: ctx
            .config
            .logs_path
            .clone()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from),
        tz: jiff::tz::TimeZone::get(&ctx.config.tz).unwrap_or(jiff::tz::TimeZone::UTC),
        tracker: ctx.tracker.clone(),
    };
    tracing::info!(target: LOG, "Pipeline created");
    Ok(Some(Arc::new(DeliveryPipeline::new(deps))))
}

/// The parcel subsystem: the forget route, the dedup entity and, when
/// configured, the `ParcelTracker` email handler (share `triage` with the
/// calendar pipeline).
pub fn subsystem(ctx: &AppContext, triage: EmailTriage) -> Result<Subsystem, ParcelSetupError> {
    let handler = handler(ctx, triage)?;
    Ok(Subsystem {
        router: router(ctx.store.clone()),
        entities: entities(),
        managed_entities: managed_entities(),
        email_handlers: handler
            .into_iter()
            .map(|h| h as Arc<dyn omni_core::email::EmailHandler>)
            .collect(),
        ..Subsystem::named("parcel")
    })
}
