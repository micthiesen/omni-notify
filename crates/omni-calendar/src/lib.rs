//! Calendar events from email: candidate filter, AI extraction and
//! sanitization, iCloud CalDAV discovery (RFC 6764) and writes, tracked-event
//! persistence, the `CalendarEvents` email handler, the calendar MCP tools and
//! the `CalendarWriter` port.
//!
//! The pipeline reaches the email core (activity, retry queue, sender rules,
//! triage, log capture) through the [`support::EmailSupport`] trait, implemented
//! over `omni_email` by [`email_core::OmniEmailSupport`], and the transport's
//! attachment download through [`support::AttachmentSource`], implemented over
//! the `EmailReader` port by [`support::PortAttachments`].

pub mod caldav;
pub mod email_core;
pub mod error;
pub mod extraction;
pub mod filter;
pub mod logfile;
pub mod mcp;
pub mod persistence;
pub mod pipeline;
pub mod primary;
pub mod routes;
pub mod support;
pub mod writer;

use std::sync::Arc;

use omni_email::triage::EmailTriage;
use omni_mcp_kit::ToolMetaError;
use omni_runtime::ports::CalendarWriter;
use omni_runtime::{AppContext, BootError, BootPhase, BootStep, ManagedEntity, Subsystem};
use omni_store::entity::EntityDescriptor;

pub use caldav::{Caldav, CaldavSettings};
pub use email_core::OmniEmailSupport;
pub use persistence::CreatedCalendarEvent;
pub use pipeline::{CalendarEventPipeline, PipelineDeps};
pub use support::{AttachmentSource, EmailSupport, PIPELINE, PortAttachments};

const LOG: &str = "Main:CalendarEvents";

/// The seams the binary supplies (see [`support`]).
#[derive(Clone)]
pub struct CalendarDeps {
    pub email: Arc<dyn EmailSupport>,
    pub attachments: Arc<dyn AttachmentSource>,
}

impl CalendarDeps {
    /// Production wiring: the email core from `omni_email` (with the triage
    /// instance shared with the parcel pipeline) plus the transport's
    /// attachment download through the `EmailReader` port.
    pub fn new(ctx: &AppContext, triage: EmailTriage) -> Self {
        Self {
            email: Arc::new(OmniEmailSupport::new(
                ctx.store.clone(),
                ctx.run_logs(),
                triage,
                ctx.pushover.clone(),
            )),
            attachments: Arc::new(PortAttachments::new(ctx.ports.clone())),
        }
    }
}

/// CalDAV access configured from the app context (credentials, side-effect
/// mode, default time zone).
pub fn caldav(ctx: &AppContext) -> Caldav {
    Caldav::new(
        ctx.http.clone(),
        ctx.clock.clone(),
        ctx.side_effects,
        CaldavSettings::from_config(&ctx.config),
        ctx.config.tz.clone(),
    )
}

/// The `CalendarWriter` port implementation (wiring sets it on `ctx.ports`).
pub fn calendar_writer(ctx: &AppContext) -> Arc<dyn CalendarWriter> {
    Arc::new(writer::CaldavCalendarWriter::new(caldav(ctx)))
}

/// The entity descriptors this crate owns.
pub fn entities() -> Vec<EntityDescriptor> {
    use primary::store::{
        ChangeRow, MirrorResource, OperationRecord, PrimaryPin, SyncState, WriteEcho,
    };
    vec![
        EntityDescriptor::of::<CreatedCalendarEvent>(),
        EntityDescriptor::of::<PrimaryPin>(),
        EntityDescriptor::of::<SyncState>(),
        EntityDescriptor::of::<MirrorResource>(),
        EntityDescriptor::of::<ChangeRow>(),
        EntityDescriptor::of::<WriteEcho>(),
        EntityDescriptor::of::<OperationRecord>(),
    ]
}

/// The primary-calendar service configured from the app context. Record-mode
/// captures join the pipeline writer's list.
pub fn primary_calendar(ctx: &AppContext, caldav: &Caldav) -> primary::PrimaryCalendar {
    primary::PrimaryCalendar::new(primary::PrimaryDeps {
        http: ctx.http.clone(),
        settings: CaldavSettings::from_config(&ctx.config),
        store: ctx.store.clone(),
        clock: ctx.clock.clone(),
        mode: ctx.side_effects,
        recorded: caldav.writer().recorded_handle(),
        default_tz: ctx.config.tz.clone(),
        tracker: Some(ctx.tracker.clone()),
        ports: ctx.ports.clone(),
    })
}

fn managed_entities() -> Vec<ManagedEntity> {
    vec![
        ManagedEntity {
            slug: "calendar-created-event",
            label: "Created calendar events",
            description: "Calendar events created from email, keyed by normalized content.",
            warning: Some(
                "This is a deduplication gate. Deleted rows may create duplicate events.",
            ),
            entity: EntityDescriptor::of::<CreatedCalendarEvent>(),
            primary_key: &["eventHash"],
            can_delete: None,
            after_delete: None,
        },
        ManagedEntity {
            slug: "calendar-primary-change",
            label: "Calendar changes",
            description: "Changes detected in the primary iCloud calendar, by sequence number.",
            warning: Some("Deleted rows are gone from the change feed."),
            entity: EntityDescriptor::of::<primary::store::ChangeRow>(),
            primary_key: &["key"],
            can_delete: None,
            after_delete: None,
        },
        ManagedEntity {
            slug: "calendar-mcp-operation",
            label: "Calendar write operations",
            description: "Idempotent calendar tool writes, keyed by the idempotency key's hash.",
            warning: Some("Deleting a row lets its idempotency key write again."),
            entity: EntityDescriptor::of::<primary::store::OperationRecord>(),
            primary_key: &["keyHash"],
            can_delete: None,
            after_delete: None,
        },
    ]
}

/// The calendar subsystem. Without iCloud CalDAV credentials the email handler
/// and the hash reconcile boot step are omitted (the MCP tools stay registered
/// and report the missing provider).
pub fn subsystem(ctx: &AppContext, deps: CalendarDeps) -> Result<Subsystem, ToolMetaError> {
    let caldav = caldav(ctx);
    let primary = primary_calendar(ctx, &caldav);
    let mut subsystem = Subsystem::named(PIPELINE);
    subsystem.mcp_tools = mcp::calendar_tools(mcp::CalendarTools {
        store: ctx.store.clone(),
        primary: primary.clone(),
    })?;
    subsystem.router = routes::router(primary.clone());
    subsystem.entities = entities();
    subsystem.managed_entities = managed_entities();

    if caldav.provider().is_none() {
        tracing::info!(target: LOG, "Disabled: no iCloud CalDAV credentials configured");
        return Ok(subsystem);
    }

    subsystem.boot_steps.push(BootStep {
        phase: BootPhase::Reconcile,
        name: "calendar-reconcile-interrupted-writes",
        run: Box::new(|ctx: AppContext| {
            Box::pin(async move {
                let settled =
                    primary::operations::reconcile_interrupted(&ctx.store, ctx.clock.now_ms())
                        .await
                        .map_err(|e| {
                            BootError::new("calendar-reconcile-interrupted-writes", e.to_string())
                        })?;
                if settled > 0 {
                    tracing::warn!(
                        target: LOG,
                        "Marked {settled} interrupted calendar write(s) uncertain or failed"
                    );
                }
                Ok(())
            })
        }),
    });
    match jiff::tz::TimeZone::get(&ctx.config.tz)
        .map_err(|e| e.to_string())
        .and_then(|tz| {
            let sync = primary::task::PrimarySyncTask::new(primary.clone(), &tz)
                .map_err(|e| e.to_string())?;
            let starting = primary::starting::StartingEventsTask::new(primary, &tz)
                .map_err(|e| e.to_string())?;
            Ok((sync, starting))
        }) {
        Ok((sync, starting)) => {
            subsystem.tasks.push(Arc::new(sync));
            subsystem.tasks.push(Arc::new(starting));
        }
        Err(error) => tracing::error!(target: LOG, "Calendar tasks not scheduled: {error}"),
    }

    subsystem.boot_steps.push(BootStep {
        phase: BootPhase::Reconcile,
        name: "calendar-reconcile-event-hashes",
        run: Box::new(|ctx: AppContext| {
            Box::pin(async move {
                let rekeyed = persistence::reconcile_event_hashes(&ctx.store)
                    .await
                    .map_err(|e| {
                        BootError::new("calendar-reconcile-event-hashes", e.to_string())
                    })?;
                if rekeyed > 0 {
                    tracing::info!(
                        target: LOG,
                        "Reconciled {rekeyed} calendar event hash(es) to new scheme"
                    );
                }
                Ok(())
            })
        }),
    });

    let pipeline = CalendarEventPipeline::new(PipelineDeps {
        config: ctx.config.clone(),
        store: ctx.store.clone(),
        clock: ctx.clock.clone(),
        ai: ctx.ai.clone(),
        pushover: ctx.pushover.clone(),
        caldav,
        support: deps.email,
        attachments: deps.attachments,
    });
    tracing::info!(target: LOG, "Pipeline created");
    subsystem.email_handlers.push(Arc::new(pipeline));
    Ok(subsystem)
}
