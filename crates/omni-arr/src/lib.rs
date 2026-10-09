//! Sonarr/Radarr import recovery (`ArrRecovery`) and Observer issue repair
//! (`ObserverRepair`).
//!
//! Both tasks reserve every mutation durably before the HTTP call, verify the
//! outcome against Arr or Observer before reporting success, and never repeat
//! a mutation whose outcome is uncertain (`docs/arr-recovery.md`,
//! `docs/observer-repair.md`).

pub mod arr_recovery;
pub mod js_opt;
mod json_api;
pub mod observer;
pub mod observer_repair;
pub mod paths;
pub mod side_effects;

use std::sync::Arc;

use omni_runtime::{AppContext, Subsystem};
use omni_store::entity::EntityDescriptor;
use omni_tasks::Task;

pub use json_api::ApiFailure;
pub use side_effects::{RecordedMutation, SideEffects};

/// Entities this package owns (for `migrate_all` and the compat audit).
pub fn entities() -> Vec<EntityDescriptor> {
    vec![
        EntityDescriptor::of::<arr_recovery::persistence::RecoveryState>(),
        EntityDescriptor::of::<observer_repair::persistence::ObserverRepairState>(),
    ]
}

/// The Arr subsystem: the `ArrRecovery` and `ObserverRepair` tasks when
/// their configuration is complete. No routes, MCP tools or services.
pub fn subsystem(ctx: &AppContext) -> Subsystem {
    let side_effects = SideEffects::new(ctx.side_effects);
    let mut tasks: Vec<Arc<dyn Task>> = Vec::new();
    if let Some(task) = arr_recovery::task::ArrRecoveryTask::create(ctx, side_effects.clone()) {
        tasks.push(Arc::new(task));
    }
    if let Some(task) = observer_repair::task::ObserverRepairTask::create(ctx, side_effects) {
        tasks.push(Arc::new(task));
    }
    Subsystem {
        tasks,
        entities: entities(),
        ..Subsystem::named("arr")
    }
}

/// The configured `TZ` for cron schedules (UTC when the zone is unknown).
pub(crate) fn time_zone(config: &omni_config::Config) -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get(&config.tz).unwrap_or_else(|error| {
        tracing::warn!(target: "Main", error = %error, "Unknown TZ {}; using UTC", config.tz);
        jiff::tz::TimeZone::UTC
    })
}
