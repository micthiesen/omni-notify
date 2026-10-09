//! Server iCloud Reminders (WP10): `src/reminders/**`, `src/icloud/**` and
//! `src/mcp/tools/reminders.ts`.
//!
//! Independent of Mac EventKit, IMAP and CalDAV. Disabled (never failing boot)
//! without the complete `ICLOUD_REMINDERS_*` configuration. The `/reminders` page has
//! no extra login and exposes only bounded authentication controls and public status;
//! Reminders data and CRUD are only reachable through the bearer-authenticated MCP
//! tools. Apple terms are never accepted and ADP is never disabled. See
//! docs/server-reminders.md.

pub mod apple;
pub mod cloudkit;
pub mod cloudkit_extras;
pub mod codec;
pub mod config;
pub mod cookies;
mod json;
pub mod mcp;
pub mod page;
pub mod protected_access;
pub mod recurrence;
pub mod recurring_completion;
pub mod routes;
pub mod service;
pub mod store;
pub mod task;

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_alerts::{PushoverChannel, PushoverMessage};
use omni_runtime::{AppContext, BackgroundService, BootError, BootPhase, BootStep, Subsystem};

use crate::apple::{AppleApi, AppleClientOptions, AppleRemindersClient, HttpAppleTransport};
use crate::config::RemindersConfiguration;
use crate::service::{RemindersService, ServiceDeps};
use crate::store::FileRemindersStore;

const LOG: &str = "Reminders";

/// Builds the account owner from the application context.
pub fn build_service(ctx: &AppContext) -> RemindersService {
    let config =
        RemindersConfiguration::from_config(&ctx.config, ctx.paths.reminders_private.clone());
    let account = config.account.clone().unwrap_or_default();
    let password = config.password.clone().unwrap_or_default();
    let store = Arc::new(FileRemindersStore::new(
        config.directory.clone(),
        config.storage_key.as_deref().unwrap_or(""),
        &account,
    ));
    let transport = Arc::new(HttpAppleTransport::new(ctx.http.clone()));
    let (clock, side_effects) = (ctx.clock.clone(), ctx.side_effects);
    let apple = Box::new(move |storage| {
        Arc::new(AppleRemindersClient::new(AppleClientOptions {
            account,
            password,
            storage,
            transport,
            clock,
            timeout: None,
            side_effects,
        })) as Arc<dyn AppleApi>
    });
    let pushover = ctx.pushover.clone();
    let url = format!(
        "{}/reminders",
        config.public_origin.as_deref().unwrap_or_default()
    );
    let notify: service::Notifier = Arc::new(move || {
        let pushover = pushover.clone();
        let url = url.clone();
        Box::pin(async move {
            pushover
                .send(
                    PushoverChannel::General,
                    PushoverMessage {
                        message: "Open Omni to restore Reminders access. Apple may require a verification code or trusted-device approval.".into(),
                        title: Some("iCloud Reminders needs attention".into()),
                        url: Some(url),
                        url_title: Some("Open Reminders".into()),
                        priority: None,
                        sound: None,
                        timestamp: None,
                    },
                )
                .await
                .map(|_| ())
                .map_err(|error| {
                    tracing::warn!(target: LOG, %error, "Reminders notification failed");
                })
        }) as BoxFuture<'static, Result<(), ()>>
    });
    let tracker = ctx.tracker.clone();
    let background: service::Background = Arc::new(move |job| {
        omni_core::spawn::spawn_tracked(&tracker, "reminders-index", job);
    });
    RemindersService::new(
        &config,
        ServiceDeps {
            store,
            apple,
            notify,
            log_failure: Some(Arc::new(|diagnostic| {
                tracing::warn!(
                    target: LOG,
                    stage = ?diagnostic.stage,
                    category = ?diagnostic.category,
                    http_status = ?diagnostic.http_status,
                    "iCloud Reminders request failed"
                );
            })),
            background: Some(background),
            tracker: Some(ctx.tracker.clone()),
            clock: ctx.clock.clone(),
        },
    )
}

/// Failures assembling the subsystem (invalid timezone or golden tool metadata).
#[derive(Debug, thiserror::Error)]
pub enum SubsystemError {
    #[error("invalid TZ for RemindersSession: {0}")]
    TimeZone(String),
    #[error(transparent)]
    Schedule(#[from] omni_tasks::InvalidScheduleError),
    #[error(transparent)]
    Tools(#[from] omni_mcp_kit::ToolMetaError),
}

/// The Reminders subsystem: admin routes, the code page, the `RemindersSession`
/// task, a boot-time session check and the MCP tools. No docstore entities.
pub fn subsystem(ctx: &AppContext) -> Result<Subsystem, SubsystemError> {
    let service = build_service(ctx);
    subsystem_with(ctx, service)
}

/// [`subsystem`] over an existing service (tests and alternative wiring).
pub fn subsystem_with(
    ctx: &AppContext,
    service: RemindersService,
) -> Result<Subsystem, SubsystemError> {
    let tz = jiff::tz::TimeZone::get(&ctx.config.tz)
        .map_err(|e| SubsystemError::TimeZone(e.to_string()))?;
    let router = routes::reminders_router(
        Arc::new(service.clone()),
        ctx.config.icloud_reminders_public_origin.as_deref(),
        ctx.clock.clone(),
    )
    .merge(page::page_router());
    let task = task::RemindersSessionTask::new(service.clone(), &tz)?;
    let mcp_tools = mcp::reminders_tools(service.clone())?;
    let log_service = service.clone();
    let boot_log = BootStep {
        phase: BootPhase::Services,
        name: "reminders-status",
        run: Box::new(move |_ctx| {
            Box::pin(async move {
                let phase = serde_json::to_value(log_service.status().phase)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                tracing::info!(target: LOG, "Server iCloud Reminders: {phase}");
                Ok::<(), BootError>(())
            })
        }),
    };
    let check_service = service;
    let boot_check = BackgroundService {
        name: "reminders-boot-check",
        start: Box::new(move |_ctx| {
            let service = check_service.clone();
            Box::pin(async move { service.health_check().await })
        }),
        retry: None,
    };
    Ok(Subsystem {
        name: "reminders",
        router,
        tasks: vec![Arc::new(task)],
        mcp_tools,
        boot_steps: vec![boot_log],
        services: vec![boot_check],
        ..Subsystem::named("reminders")
    })
}
