//! Fit-first personal stylist ledger (see docs/stylist.md).
//!
//! A case-based, single-person ledger: typed records with revisions, immutable
//! events, private evidence, frozen predictions scored by later outcomes,
//! narrow regression-checked rules, a versioned runbook, and verified export
//! and restore. Every tool is namespaced: `real` is the owner's ledger and
//! `fixture-*` namespaces hold disposable test data.

pub mod api;
pub mod archive;
pub mod backup;
pub mod context;
pub mod error;
pub mod evidence;
pub mod fit;
pub mod ledger;
pub mod mcp;
pub mod model;
pub mod runbook;
pub mod service;
pub mod state;
pub mod types;
pub mod units;
pub mod validate;

use std::sync::Arc;

use omni_runtime::{AppContext, BootError, BootPhase, BootStep, Subsystem};

pub use error::StylistError;
pub use service::Stylist;

const LOG: &str = "Stylist";

#[derive(Debug, thiserror::Error)]
pub enum SubsystemError {
    #[error(transparent)]
    Tool(#[from] omni_mcp_kit::ToolMetaError),
    #[error(transparent)]
    Schedule(#[from] omni_tasks::InvalidScheduleError),
}

/// The owner's time zone (config `TZ`, UTC when unknown).
fn time_zone(ctx: &AppContext) -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get(&ctx.config.tz).unwrap_or(jiff::tz::TimeZone::UTC)
}

/// The service over the app's store, clock and private directory.
pub fn service(ctx: &AppContext) -> Stylist {
    Stylist::new(
        ctx.store.clone(),
        ctx.clock.clone(),
        time_zone(ctx),
        ctx.paths.stylist_private.clone(),
    )
}

pub fn subsystem(ctx: &AppContext) -> Result<Subsystem, SubsystemError> {
    let stylist = service(ctx);
    let init = stylist.clone();
    let tasks: Vec<Arc<dyn omni_tasks::Task>> = vec![Arc::new(backup::BackupTask::new(
        stylist.clone(),
        &time_zone(ctx),
    )?)];
    Ok(Subsystem {
        tasks,
        mcp_tools: mcp::tools(Arc::new(stylist))?,
        // A failure here must not stop the rest of Omni: the tools retry the
        // same initialization on every call and report the error.
        boot_steps: vec![BootStep {
            phase: BootPhase::Migrate,
            name: "stylist-ledger",
            run: Box::new(move |_ctx| {
                Box::pin(async move {
                    if let Err(error) = init.ready().await {
                        tracing::warn!(target: LOG, "Stylist ledger unavailable: {}", error.tool_text());
                    }
                    Ok::<(), BootError>(())
                })
            }),
        }],
        ..Subsystem::named("stylist")
    })
}
