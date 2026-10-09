//! Scheduled briefing agents (WP11; `src/briefing-agent/**`).
//!
//! One task per `BRIEFINGS_PATH/<Name>.md` (front matter `schedule`, body =
//! prompt with `{{history:N}}`, `{{date}}`, `{{time}}` placeholders). Each run
//! researches with `web_search` / `fetch_url` and pushes through the
//! `send_notification` tool, whose deliveries are reserved durably before
//! Pushover and released only on a confirmed failure. Implements the
//! `BriefingsReader` port and serves `GET /api/briefings`.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use omni_ai::tools::{FetchUrl, WebSearch};
use omni_api::briefings::BriefingsResponse;
use omni_runtime::{AppContext, ManagedEntity, Subsystem};
use omni_server_kit::{ApiError, ApiResult};
use omni_store::Store;
use omni_store::entity::EntityDescriptor;

pub mod configs;
pub mod format;
pub mod logfile;
pub mod persistence;
pub mod placeholders;
pub mod reader;
pub mod task;

pub use configs::{BriefingConfig, ConfigLoadError, load_briefing_configs};
pub use persistence::{BriefingDeliveryData, BriefingHistoryData, BriefingNotificationData};
pub use reader::StoreBriefingsReader;
pub use task::{BriefingDeps, BriefingNotifier, BriefingTask, PushoverBriefingNotifier};

/// Subsystem construction failures (boot fails like the TS loader would).
#[derive(Debug, thiserror::Error)]
pub enum BriefingsError {
    #[error(transparent)]
    Load(#[from] ConfigLoadError),
    #[error("invalid TZ {tz:?}: {reason}")]
    TimeZone { tz: String, reason: String },
}

/// Entities owned by this crate (for `migrate_all` and the compat audit).
pub fn entities() -> Vec<EntityDescriptor> {
    vec![
        EntityDescriptor::of::<BriefingHistoryData>(),
        EntityDescriptor::of::<BriefingDeliveryData>(),
    ]
}

/// Data-manager rows (`src/data-manager.ts`).
pub fn managed_entities() -> Vec<ManagedEntity> {
    vec![ManagedEntity {
        slug: "briefing-history",
        label: "Briefing history",
        description: "Recent notifications retained for briefing deduplication.",
        warning: Some("Deleting history can allow a briefing to repeat prior stories."),
        entity: EntityDescriptor::of::<BriefingHistoryData>(),
        primary_key: &["briefingName"],
        can_delete: None,
        after_delete: None,
    }]
}

/// The `BriefingsReader` WP14 installs with `ctx.ports.set_briefings_reader`.
pub fn briefings_reader(store: Store) -> Arc<dyn omni_runtime::ports::BriefingsReader> {
    Arc::new(StoreBriefingsReader::new(store))
}

/// `GET /api/briefings`.
pub fn router(store: Store) -> Router {
    Router::new()
        .route("/api/briefings", get(list_briefings))
        .with_state(store)
}

async fn list_briefings(State(store): State<Store>) -> ApiResult<BriefingsResponse> {
    let briefings = reader::briefing_summaries(&store)
        .await
        .map_err(ApiError::internal)?;
    Ok(axum::Json(BriefingsResponse { briefings }))
}

/// Production dependencies for briefing tasks.
pub fn deps(ctx: &AppContext) -> Result<BriefingDeps, BriefingsError> {
    let tz = jiff::tz::TimeZone::get(&ctx.config.tz).map_err(|e| BriefingsError::TimeZone {
        tz: ctx.config.tz.clone(),
        reason: e.to_string(),
    })?;
    let key = ctx.config.tavily_api_key.clone().unwrap_or_default();
    Ok(BriefingDeps {
        store: ctx.store.clone(),
        clock: ctx.clock.clone(),
        ai: ctx.ai.clone(),
        config: ctx.config.clone(),
        tz,
        notifier: Arc::new(PushoverBriefingNotifier(ctx.pushover.clone())),
        web_search: Arc::new(WebSearch::new(
            ctx.public_http.clone(),
            key,
            ctx.costs.clone(),
        )),
        fetch_url: Arc::new(FetchUrl::new(ctx.public_http.clone())),
        logs_path: ctx
            .config
            .logs_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from),
    })
}

/// The briefings subsystem: one task per loaded config (only with
/// `TAVILY_API_KEY`), the route, entities and the data-manager row. WP14 also
/// installs [`briefings_reader`] as the `BriefingsReader` port.
pub fn subsystem(ctx: &AppContext) -> Result<Subsystem, BriefingsError> {
    let deps = deps(ctx)?;
    let configs = load_briefing_configs(ctx.config.briefings_path.as_deref(), &deps.tz)?;
    let tasks = configs
        .into_iter()
        .filter_map(|config| BriefingTask::create(config, deps.clone()))
        .map(|task| Arc::new(task) as Arc<dyn omni_tasks::Task>)
        .collect();
    Ok(Subsystem {
        name: "briefings",
        router: router(ctx.store.clone()),
        tasks,
        entities: entities(),
        managed_entities: managed_entities(),
        ..Subsystem::default()
    })
}
