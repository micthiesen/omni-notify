//! MCP server, MCP Events and MCP activity.
//!
//! - [`endpoint`] / [`rpc`]: the authenticated `ALL /mcp` endpoint serving
//!   every package's tools in both protocol eras, plus `events/*`;
//! - [`events`]: the shared durable outbox, webhook delivery, Executor
//!   delegated authorization and the Claude session watcher;
//! - [`activity`] / [`activity_routes`]: MCP call recording and the
//!   `/api/mcp/activity` and `/api/claude/*` routes;
//! - [`tools`]: the `system`, `events` and `claude-sessions` tool groups;
//! - [`policy`]: the `docs/mcp-policy.json` inventory from registered tools.
//!
//! # Wiring
//!
//! 1. Build `omni_device_link::DeviceLink::from_context` first (it sets the
//!    [`host::ClaudeHost`] port when `OMNI_DEVICE_LINK_TOKEN` is set), set the
//!    `ArchiveEcho` port (`omni_imap::transport::StoreArchiveEcho`), then build
//!    [`McpPackage::new`].
//! 2. Register [`McpPackage::email_handler`] first on the email dispatcher,
//!    and set the `EventPublisher` port from [`McpPackage::event_publisher`].
//! 3. Collect every other subsystem's `mcp_tools`, append
//!    [`McpPackage::tools`], and call [`McpPackage::subsystem`]; it serves
//!    them in [`tools::TOOL_ORDER`] and fails when the set differs from that
//!    list. The returned subsystem carries the routes, tasks, entities,
//!    boot step and delivery worker; its `mcp_tools` is empty because the
//!    endpoint already serves every tool.

pub mod activity;
pub mod activity_routes;
pub mod endpoint;
pub mod events;
pub mod host;
pub mod json;
pub mod policy;
pub mod rpc;
pub mod tasks;
pub mod tools;

use std::sync::Arc;

use futures::FutureExt as _;
use omni_core::email::EmailHandler;
use omni_mcp_kit::{McpTool, ToolMetaError};
use omni_runtime::{AppContext, BackgroundService, BootError, BootPhase, BootStep, Subsystem};
use omni_store::entity::EntityDescriptor;
use omni_tasks::CronSchedule;

use crate::activity::{ActivityRecorder, McpCallData};
use crate::events::claude_sessions::{ClaudeSessionWatch, ClaudeSessionWatcher};
use crate::events::executor_auth::{EventAuthorizer, ExecutorEventAuthorizer};
use crate::events::persistence::{EventDelivery, EventReceipt, EventRequest, EventSubscription};
use crate::events::publisher::McpEventPublisher;
use crate::events::service::McpEventService;
use crate::events::task_runs::TaskRunWatcher;
use crate::events::webhook::{LiveWebhookTransport, WebhookClient};
use crate::host::ClaudeHost;
use crate::rpc::{McpProtocol, ProtocolSetupError};
use crate::tasks::{
    CLAUDE_SESSION_EVENTS_SCHEDULE, ClaudeSessionEventsTask, MCP_EVENT_DELIVERY_SCHEDULE,
    McpEventDeliveryTask, TASK_RUN_EVENTS_SCHEDULE, TaskRunEventsTask,
};
use crate::tools::claude_sessions::ClaudeDeps;
use crate::tools::system::{ConfiguredFeatures, SystemDeps};

const LOG: &str = "MCP";
/// Executor's in-cluster MCP session endpoint, used when Dockerized without an override.
pub const DEFAULT_EXECUTOR_AUTH_URL: &str = "http://executor:4788/api/auth/mcp/get-session";

/// Every entity this package persists (for `migrate_all` and the compat audit).
pub fn entities() -> Vec<EntityDescriptor> {
    vec![
        EntityDescriptor::of::<McpCallData>(),
        EntityDescriptor::of::<EventSubscription>(),
        EntityDescriptor::of::<EventReceipt>(),
        EntityDescriptor::of::<EventDelivery>(),
        EntityDescriptor::of::<EventRequest>(),
        EntityDescriptor::of::<ClaudeSessionWatch>(),
    ]
}

#[derive(Debug, thiserror::Error)]
pub enum McpSetupError {
    #[error(transparent)]
    ToolMeta(#[from] ToolMetaError),
    #[error(transparent)]
    Protocol(#[from] ProtocolSetupError),
    #[error(transparent)]
    ExecutorAuthUrl(#[from] crate::events::executor_auth::InvalidSessionUrl),
    #[error("invalid schedule {0}")]
    Schedule(String),
    #[error(
        "OMNI_DEVICE_LINK_TOKEN is set but the Claude Code host port is not; build the device link first"
    )]
    MissingClaudeHost,
}

fn non_empty(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|v| !v.is_empty())
}

fn time_zone(config: &omni_config::Config) -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get(&config.tz).unwrap_or(jiff::tz::TimeZone::UTC)
}

/// The MCP package for one process.
pub struct McpPackage {
    ctx: AppContext,
    token: Option<String>,
    events: Option<McpEventService>,
    watcher: Option<ClaudeSessionWatcher>,
    host: Option<Arc<dyn ClaudeHost>>,
    recorder: ActivityRecorder,
    features: ConfiguredFeatures,
}

impl McpPackage {
    /// Fails boot when `OMNI_EVENTS_EXECUTOR_AUTH_URL` is not a
    /// credential-free HTTP(S) URL, and when a configured device link has not
    /// set the `ClaudeHost` port yet.
    pub fn new(ctx: &AppContext) -> Result<Self, McpSetupError> {
        let config = &ctx.config;
        let host: Option<Arc<dyn ClaudeHost>> = ctx.ports.claude_host();
        let device_link_configured = non_empty(config.omni_device_link_token.as_ref())
            .is_some_and(|token| non_empty(config.omni_mcp_token.as_ref()) != Some(token));
        if device_link_configured && host.is_none() {
            return Err(McpSetupError::MissingClaudeHost);
        }
        let token = non_empty(config.omni_mcp_token.as_ref()).map(str::to_owned);
        let auth_url = non_empty(config.omni_events_executor_auth_url.as_ref())
            .map(str::to_owned)
            .or_else(|| {
                config
                    .dockerized
                    .then(|| DEFAULT_EXECUTOR_AUTH_URL.to_owned())
            });
        let authorizer: Option<Arc<dyn EventAuthorizer>> = match (&token, auth_url.as_deref()) {
            (Some(_), Some(url)) => Some(Arc::new(
                ExecutorEventAuthorizer::new(url, ctx.http.clone(), ctx.clock.clone())?
                    .with_time_zone(time_zone(config)),
            )),
            _ => None,
        };
        let events = token.as_deref().map(|token| {
            let transport = LiveWebhookTransport::new(ctx.public_http.clone(), ctx.side_effects);
            McpEventService::new(
                token,
                ctx.store.clone(),
                ctx.clock.clone(),
                Arc::new(WebhookClient::new(Arc::new(transport), ctx.clock.clone())),
                authorizer,
                ctx.ports.clone(),
            )
        });
        let watcher = match (&events, &host) {
            (Some(events), Some(host)) => Some(ClaudeSessionWatcher::new(
                events.clone(),
                host.clone(),
                ctx.store.clone(),
                ctx.clock.clone(),
            )),
            _ => None,
        };
        Ok(Self {
            recorder: ActivityRecorder::new(
                ctx.store.clone(),
                ctx.clock.clone(),
                ctx.tracker.clone(),
            ),
            features: ConfiguredFeatures::from_config(config),
            ctx: ctx.clone(),
            token,
            events,
            watcher,
            host,
        })
    }

    /// Overrides the configuration-derived `system_status` facts.
    pub fn with_features(mut self, features: ConfiguredFeatures) -> Self {
        self.features = features;
        self
    }

    pub fn events(&self) -> Option<&McpEventService> {
        self.events.as_ref()
    }

    pub fn watcher(&self) -> Option<&ClaudeSessionWatcher> {
        self.watcher.as_ref()
    }

    /// The `EventPublisher` port over this outbox, when MCP Events are enabled.
    pub fn event_publisher(&self) -> Option<McpEventPublisher> {
        self.events.clone().map(McpEventPublisher::new)
    }

    /// The `McpEvents` email handler (register it before every other handler).
    pub fn email_handler(&self) -> Option<Arc<dyn EmailHandler>> {
        self.events.as_ref().map(McpEventService::email_handler)
    }

    /// This package's tools: `system`, `events`, `claude-sessions` groups.
    pub fn tools(&self) -> Result<Vec<McpTool>, ToolMetaError> {
        let mut tools = tools::system::system_tools(&SystemDeps {
            tasks: Arc::new(self.ctx.tasks.clone()),
            ports: self.ctx.ports.clone(),
            features: self.features,
            clock: self.ctx.clock.clone(),
        })?;
        tools.extend(tools::events::event_tools(self.events.clone())?);
        tools.extend(tools::claude_sessions::claude_session_tools(&ClaudeDeps {
            host: self.host.clone(),
            watcher: self
                .watcher
                .clone()
                .map(|w| Arc::new(w) as Arc<dyn omni_runtime::ports::ClaudeSessionNotifier>),
        })?);
        Ok(tools)
    }

    /// The subsystem serving `all_tools` (every package's, this one's included).
    pub fn subsystem(self, all_tools: Vec<McpTool>) -> Result<Subsystem, McpSetupError> {
        let ctx = &self.ctx;
        let protocol = match &self.token {
            Some(_) => Some(McpProtocol::new(
                all_tools,
                self.recorder.clone(),
                self.events.clone(),
            )?),
            None => None,
        };
        let router = endpoint::router(self.token.as_deref(), protocol).merge(
            activity_routes::router(ctx.store.clone(), ctx.clock.clone(), self.host.clone()),
        );
        let tz = time_zone(&ctx.config);
        let schedule = |expr: &str| {
            CronSchedule::parse(expr, &tz).map_err(|_| McpSetupError::Schedule(expr.to_owned()))
        };
        let mut subsystem = Subsystem {
            router,
            entities: entities(),
            ..Subsystem::named("mcp")
        };
        subsystem.boot_steps.push(BootStep {
            phase: BootPhase::Reconcile,
            name: "markInterruptedCalls",
            run: Box::new(|ctx: AppContext| {
                async move {
                    let marked = activity::mark_interrupted_calls(&ctx.store, ctx.clock.now_ms())
                        .await
                        .map_err(|e| BootError::new("markInterruptedCalls", e.to_string()))?;
                    if marked > 0 {
                        tracing::warn!(target: "Main", "Marked {marked} interrupted MCP call(s)");
                    }
                    Ok(())
                }
                .boxed()
            }),
        });
        if let Some(events) = &self.events {
            subsystem.boot_steps.push(BootStep {
                phase: BootPhase::Migrate,
                name: "requireArchiveEcho",
                run: Box::new(|ctx: AppContext| {
                    async move {
                        if ctx.ports.archive_echo().is_some() {
                            Ok(())
                        } else {
                            Err(BootError::new(
                                "requireArchiveEcho",
                                "MCP email events need the ArchiveEcho port \
                                 (omni_imap::transport::StoreArchiveEcho); refusing to start \
                                 rather than publish an event for every archive move",
                            ))
                        }
                    }
                    .boxed()
                }),
            });
            subsystem.tasks.push(Arc::new(McpEventDeliveryTask::new(
                events.clone(),
                schedule(MCP_EVENT_DELIVERY_SCHEDULE)?,
            )));
            subsystem.tasks.push(Arc::new(TaskRunEventsTask::new(
                TaskRunWatcher::new(events.clone(), ctx.store.clone(), ctx.clock.clone()),
                schedule(TASK_RUN_EVENTS_SCHEDULE)?,
            )));
            let worker = events.clone();
            subsystem.services.push(BackgroundService {
                name: "McpEventDelivery worker",
                start: Box::new(move |ctx: AppContext| {
                    let events = worker.clone();
                    async move {
                        if events.drain().await.is_err() {
                            tracing::warn!(target: LOG, "MCP event recovery deferred to scheduled retry");
                        }
                        events.delivery_worker(ctx.shutdown.clone()).await;
                    }
                    .boxed()
                }),
                retry: None,
            });
        }
        if let Some(watcher) = &self.watcher {
            subsystem.tasks.push(Arc::new(ClaudeSessionEventsTask::new(
                watcher.clone(),
                schedule(CLAUDE_SESSION_EVENTS_SCHEDULE)?,
            )));
        }
        Ok(subsystem)
    }
}
