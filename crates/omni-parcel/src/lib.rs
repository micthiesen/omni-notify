//! Parcel tracker email pipeline: candidate filter, tracking-number
//! extraction, carrier validation against the live Parcel list, durable
//! dedup with reservation before submission, and the delivery-forget route.
//! Also the budgeted, cached delivery-status read (`deliveries`) behind
//! `GET /api/parcels` and the `parcels_list` / `parcels_get` MCP tools.

pub mod carriers;
pub mod deliveries;
pub mod error;
pub mod extraction;
pub mod filter;
pub mod log_file;
pub mod mcp;
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
use crate::deliveries::api::DeliveriesClient;
use crate::deliveries::state::{DeliveriesSnapshot, ReadState};
use crate::deliveries::task::{DeliveriesTask, DeliveriesTaskDeps};
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
    #[error(transparent)]
    Schedule(#[from] omni_tasks::InvalidScheduleError),
    #[error(transparent)]
    ToolMeta(#[from] omni_mcp_kit::ToolMetaError),
}

pub fn entities() -> Vec<EntityDescriptor> {
    vec![
        EntityDescriptor::of::<SubmittedDelivery>(),
        EntityDescriptor::of::<DeliveriesSnapshot>(),
        EntityDescriptor::of::<ReadState>(),
    ]
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

fn api_key(ctx: &AppContext) -> Option<String> {
    ctx.config
        .parcel_api_key
        .clone()
        .filter(|key| !key.is_empty())
}

/// `None` (logged) without `PARCEL_API_KEY`.
pub fn handler(
    ctx: &AppContext,
    triage: EmailTriage,
) -> Result<Option<Arc<DeliveryPipeline>>, ParcelSetupError> {
    let Some(api_key) = api_key(ctx) else {
        tracing::info!(target: LOG, "Disabled: missing PARCEL_API_KEY");
        return Ok(None);
    };
    let carriers = Arc::new(CarrierDirectory::new(
        PublicHttpClient::new(&ctx.http),
        ctx.clock.clone(),
    )?);
    pipeline(ctx, triage, api_key, carriers).map(Some)
}

fn pipeline(
    ctx: &AppContext,
    triage: EmailTriage,
    api_key: String,
    carriers: Arc<CarrierDirectory>,
) -> Result<Arc<DeliveryPipeline>, ParcelSetupError> {
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
        systemic: omni_email::systemic::SystemicReporter::new(
            ctx.store.clone(),
            ctx.pushover.clone(),
        ),
    };
    tracing::info!(target: LOG, "Pipeline created");
    Ok(Arc::new(DeliveryPipeline::new(deps)))
}

/// The `ParcelDeliveries` read task; shares the carrier list with the pipeline.
pub fn deliveries_task(
    ctx: &AppContext,
    api_key: String,
    carriers: Arc<CarrierDirectory>,
) -> Result<DeliveriesTask, ParcelSetupError> {
    Ok(DeliveriesTask::new(DeliveriesTaskDeps {
        store: ctx.store.clone(),
        client: DeliveriesClient::new(ctx.http.clone(), api_key)?,
        carriers,
        clock: ctx.clock.clone(),
        tracker: ctx.tracker.clone(),
        mode: ctx.side_effects,
        tz: jiff::tz::TimeZone::get(&ctx.config.tz).unwrap_or(jiff::tz::TimeZone::UTC),
    })?)
}

/// The parcel subsystem: the forget and deliveries routes, the Parcel MCP
/// tools, its entities and, when configured, the `ParcelTracker` email
/// handler (share `triage` with the calendar pipeline) and the
/// `ParcelDeliveries` read task.
pub fn subsystem(ctx: &AppContext, triage: EmailTriage) -> Result<Subsystem, ParcelSetupError> {
    let key = api_key(ctx);
    let configured = key.is_some();
    let mut email_handlers: Vec<Arc<dyn omni_core::email::EmailHandler>> = Vec::new();
    let mut tasks: Vec<Arc<dyn omni_tasks::Task>> = Vec::new();
    match key {
        Some(api_key) => {
            let carriers = Arc::new(CarrierDirectory::new(
                PublicHttpClient::new(&ctx.http),
                ctx.clock.clone(),
            )?);
            email_handlers.push(pipeline(ctx, triage, api_key.clone(), carriers.clone())?);
            tasks.push(Arc::new(deliveries_task(ctx, api_key, carriers)?));
        }
        None => tracing::info!(target: LOG, "Disabled: missing PARCEL_API_KEY"),
    }
    Ok(Subsystem {
        router: router(ctx.store.clone()).merge(deliveries::router(ctx.store.clone(), configured)),
        tasks,
        mcp_tools: mcp::tools(ctx.store.clone(), configured)?,
        entities: entities(),
        managed_entities: managed_entities(),
        email_handlers,
        ..Subsystem::named("parcel")
    })
}
