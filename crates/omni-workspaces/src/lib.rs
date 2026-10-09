//! Durable conversational workspaces and their MCP tools.
//!
//! Invariants carried from AGENTS.md:
//! - workspace rows change only through this service (routes, tools, tasks);
//! - research and drafting never authorize side effects: email scopes and
//!   calendar events are proposals that take effect only through
//!   [`WorkspaceService::approve_action`] (the approve route or the
//!   Executor-approved `workspace_action_approve` tool);
//! - the whole model output is planned and validated before one transaction
//!   writes it, and the user message is persisted before the model runs;
//! - notifications go through a durable outbox, at most once per row;
//! - calendar approvals use the deterministic UID `workspace-<actionId>@omni-notify`
//!   through the `CalendarWriter` port, where 412 means already created;
//! - scoped email sources are persisted before their run and marked triggered
//!   only after it succeeded;
//! - the model's response format is a strict schema without `oneOf`.

use std::sync::Arc;

use omni_ai::tools::{FetchUrl, WebSearch};
use omni_runtime::{AppContext, ManagedEntity, Subsystem};
use omni_store::entity::EntityDescriptor;
use omni_tasks::{CronSchedule, InvalidScheduleError};

pub mod actions;
pub mod definitions;
pub mod email;
pub mod engine;
pub mod entities;
pub mod error;
pub mod events;
pub mod mcp;
pub mod notifications;
pub mod persistence;
pub mod routes;
pub mod service;
pub mod task;
pub mod text;

pub use email::{EmailRunTrigger, RegistryEmailTrigger, WorkspaceEmailHandler};
pub use engine::{RunRequest, RunResult, RunTrigger, WorkspaceOutput};
pub use error::WorkspaceError;
pub use notifications::{
    NotificationDelivery, NotificationOutbox, PushoverWorkspaceNotifier, WorkspaceNotificationTask,
    WorkspaceNotifier,
};
pub use persistence::WorkspaceRepo;
pub use service::{WorkspaceDeps, WorkspaceService};
pub use task::WorkspaceTask;

/// Subsystem construction failures.
#[derive(Debug, thiserror::Error)]
pub enum WorkspacesBootError {
    #[error("invalid TZ {tz:?}: {reason}")]
    TimeZone { tz: String, reason: String },
    #[error(transparent)]
    Schedule(#[from] InvalidScheduleError),
    #[error(transparent)]
    ToolMeta(#[from] omni_mcp_kit::ToolMetaError),
}

/// Every workspace entity (for `migrate_all` and the compat audit).
pub fn entities() -> Vec<EntityDescriptor> {
    entities::descriptors()
}

fn managed(
    descriptor: EntityDescriptor,
    label: &'static str,
    description: &'static str,
    primary_key: &'static [&'static str],
) -> ManagedEntity {
    ManagedEntity {
        slug: descriptor.name,
        label,
        description,
        warning: None,
        entity: descriptor,
        primary_key,
        can_delete: None,
        after_delete: None,
    }
}

/// Data-manager rows, in its order.
pub fn managed_entities() -> Vec<ManagedEntity> {
    let d = entities::descriptors();
    let labels: [(&str, &str, &[&str]); 8] = [
        (
            "Workspace subjects",
            "Durable dossiers maintained inside Omni workspaces.",
            &["workspaceId", "subjectId"],
        ),
        (
            "Workspace artifact revisions",
            "Append-only history of workspace briefs, research, and decisions.",
            &["revisionId"],
        ),
        (
            "Workspace messages",
            "User and agent conversation attached to workspace subjects.",
            &["messageId"],
        ),
        (
            "Workspace sources",
            "Web and explicitly scoped email evidence used by workspaces.",
            &["sourceId"],
        ),
        (
            "Workspace actions",
            "Deterministic side effects awaiting or recording human approval.",
            &["actionId"],
        ),
        (
            "Workspace email scopes",
            "Explicitly approved senders and keywords eligible for ingestion.",
            &["workspaceId", "subjectId"],
        ),
        (
            "Workspace papercuts",
            "Deduplicated capability and workflow problems reported by agents.",
            &["papercutId"],
        ),
        (
            "Workspace notifications",
            "Durable Pushover delivery outbox with retry state.",
            &["notificationId"],
        ),
    ];
    d.into_iter()
        .zip(labels)
        .map(|(descriptor, (label, description, key))| managed(descriptor, label, description, key))
        .collect()
}

/// The production service over the app context.
pub fn service(ctx: &AppContext) -> WorkspaceService {
    let key = ctx.config.tavily_api_key.clone().unwrap_or_default();
    WorkspaceService::new(WorkspaceDeps {
        repo: WorkspaceRepo::new(ctx.store.clone()),
        config: ctx.config.clone(),
        ai: ctx.ai.clone(),
        web_search: Arc::new(WebSearch::new(
            ctx.public_http.clone(),
            key,
            ctx.costs.clone(),
        )),
        fetch_url: Arc::new(FetchUrl::new(ctx.public_http.clone())),
        notifier: Arc::new(PushoverWorkspaceNotifier(ctx.pushover.clone())),
        outbox: None,
        ports: ctx.ports.clone(),
        tasks: ctx.tasks.clone(),
        tracker: ctx.tracker.clone(),
    })
}

/// The workspaces subsystem over an existing service: the two workspace tasks
/// plus `WorkspaceNotifications`, the routes, the ten MCP tools, entities,
/// data-manager rows and the `Workspaces` email handler (wiring registers it
/// last, after McpEvents, ParcelTracker and CalendarEvents).
pub fn subsystem_with(
    service: WorkspaceService,
    ctx: &AppContext,
) -> Result<Subsystem, WorkspacesBootError> {
    let tz =
        jiff::tz::TimeZone::get(&ctx.config.tz).map_err(|e| WorkspacesBootError::TimeZone {
            tz: ctx.config.tz.clone(),
            reason: e.to_string(),
        })?;
    let mut tasks: Vec<Arc<dyn omni_tasks::Task>> = Vec::new();
    for definition in service.definitions() {
        let schedule = CronSchedule::parse(&definition.schedule, &tz)?;
        tasks.push(Arc::new(WorkspaceTask::new(
            service.clone(),
            definition.clone(),
            schedule,
        )));
    }
    tasks.push(Arc::new(WorkspaceNotificationTask::new(
        service.repo().clone(),
        service.delivery().clone(),
        CronSchedule::parse(WorkspaceNotificationTask::SCHEDULE, &tz)?,
    )));
    let trigger = RegistryEmailTrigger {
        tasks: ctx.tasks.clone(),
        workspaces: service
            .definitions()
            .iter()
            .map(|d| (d.id.clone(), d.task_name.clone()))
            .collect(),
    };
    Ok(Subsystem {
        name: "workspaces",
        router: routes::router(service.clone()),
        tasks,
        mcp_tools: mcp::tools(&service)?,
        entities: entities(),
        managed_entities: managed_entities(),
        email_handlers: vec![Arc::new(WorkspaceEmailHandler::new(
            service.repo().clone(),
            Arc::new(trigger),
        ))],
        ..Subsystem::default()
    })
}

/// [`subsystem_with`] over the production [`service`].
pub fn subsystem(ctx: &AppContext) -> Result<Subsystem, WorkspacesBootError> {
    subsystem_with(service(ctx), ctx)
}
