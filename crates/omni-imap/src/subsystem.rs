//! Composition for WP14: the IMAP transport (when iCloud credentials are set),
//! the `EmailArchive` task, the nine WP01 MCP tools, the cursor entity, the
//! data-manager row, the post-start archive recovery, and the port
//! implementations the binary installs.

use std::sync::Arc;

use omni_core::mail_source::MailSource;
use omni_runtime::ports::{ArchiveEcho, EmailReader};
use omni_runtime::{AppContext, BackgroundService, ManagedEntity, Subsystem};
use omni_store::entity::EntityDescriptor;

use crate::archive_service::ArchiveService;
use crate::compose::{ComposeMailbox, ComposeSender, ComposeService, MailerSender};
use crate::cursor::ImapFolderCursor;
use crate::map_message::SharedEnricher;
use crate::mcp_tools::{ArchiveTransport, AttachmentReader, ToolDeps, email_tools};
use crate::protocol::connect::{Connector, ImapCredentials, TlsImapConnector};
use crate::task::EmailArchiveTask;
use crate::transport::{ImapTransport, StoreArchiveEcho, TransportOptions};

const LOG: &str = "Main";

#[derive(Debug, thiserror::Error)]
pub enum ImapSubsystemError {
    #[error(transparent)]
    Tool(#[from] omni_mcp_kit::ToolMetaError),
    #[error(transparent)]
    Schedule(#[from] omni_tasks::InvalidScheduleError),
}

/// Handles WP14 wires: ports, the dispatcher's mail source, and services.
#[derive(Clone)]
pub struct ImapHandles {
    /// `None` without `ICLOUD_USERNAME` + `ICLOUD_APP_PASSWORD` (email features disabled).
    pub transport: Option<ImapTransport>,
    pub archive: ArchiveService,
    pub compose: ComposeService,
}

impl ImapHandles {
    /// For `Ports::set_email_reader`.
    pub fn email_reader(&self) -> Option<Arc<dyn EmailReader>> {
        self.transport
            .clone()
            .map(|t| Arc::new(t) as Arc<dyn EmailReader>)
    }

    /// For the email dispatcher (`omni_email::EmailDispatcher::new`).
    pub fn mail_source(&self) -> Option<Arc<dyn MailSource>> {
        self.transport
            .clone()
            .map(|t| Arc::new(t) as Arc<dyn MailSource>)
    }

    /// For `Ports::set_archive_echo` (receipts only; works without a transport).
    pub fn archive_echo(&self) -> Arc<dyn ArchiveEcho> {
        Arc::new(StoreArchiveEcho::new(self.archive.store().clone()))
    }
}

pub fn entities() -> Vec<EntityDescriptor> {
    vec![EntityDescriptor::of::<ImapFolderCursor>()]
}

pub fn managed_entities() -> Vec<ManagedEntity> {
    let entity = EntityDescriptor::of::<ImapFolderCursor>();
    vec![ManagedEntity {
        slug: entity.name,
        label: "Email IMAP cursors",
        description: "Per-folder iCloud IMAP UID cursors for incremental email processing.",
        warning: Some("Deleting a cursor can replay old email through every email handler."),
        entity,
        primary_key: &["folder"],
        can_delete: None,
        after_delete: None,
    }]
}

/// Builds the subsystem. `enricher` is omni-email's body rendering
/// (`html_to_text`, `extract_interesting_links`, `extract_email_link_metadata`);
/// `connector` overrides the iCloud TLS connector (tests).
pub fn subsystem(
    ctx: &AppContext,
    enricher: SharedEnricher,
    connector: Option<Arc<dyn Connector>>,
) -> Result<(Subsystem, ImapHandles), ImapSubsystemError> {
    let credentials = match (&ctx.config.icloud_username, &ctx.config.icloud_app_password) {
        (Some(user), Some(pass)) if !user.is_empty() && !pass.is_empty() => Some(ImapCredentials {
            user: user.clone(),
            pass: pass.clone(),
        }),
        _ => None,
    };
    let connector = connector.or_else(|| {
        credentials
            .clone()
            .map(|c| Arc::new(TlsImapConnector::icloud(c)) as Arc<dyn Connector>)
    });
    let transport = connector
        .filter(|_| credentials.is_some())
        .map(|connector| {
            ImapTransport::new(TransportOptions {
                connector,
                store: ctx.store.clone(),
                clock: ctx.clock.clone(),
                enricher,
                mode: ctx.side_effects,
                tracker: ctx.tracker.clone(),
            })
        });
    if transport.is_none() {
        tracing::info!(
            target: LOG,
            "Email features disabled: no transport configured (ICLOUD_USERNAME + ICLOUD_APP_PASSWORD)"
        );
    }
    let archive = ArchiveService::new(ctx.store.clone(), ctx.clock.clone());
    let sender = ctx
        .mailer
        .clone()
        .map(|mailer| Arc::new(MailerSender::new(mailer)) as Arc<dyn ComposeSender>);
    let compose = ComposeService::new(
        ctx.store.clone(),
        ctx.clock.clone(),
        sender,
        transport
            .clone()
            .map(|t| Arc::new(t) as Arc<dyn ComposeMailbox>),
    )
    .with_attachment_reader(
        transport
            .clone()
            .map(|t| Arc::new(t) as Arc<dyn AttachmentReader>),
    );
    let tools = email_tools(ToolDeps {
        compose: compose.clone(),
        archive: archive.clone(),
        archive_transport: transport
            .clone()
            .map(|t| Arc::new(t) as Arc<dyn ArchiveTransport>),
        attachments: transport
            .clone()
            .map(|t| Arc::new(t) as Arc<dyn AttachmentReader>),
        tracker: ctx.tracker.clone(),
    })?;

    let mut tasks = Vec::new();
    let mut services = Vec::new();
    if let Some(transport) = &transport {
        let tz = jiff::tz::TimeZone::get(&ctx.config.tz).unwrap_or(jiff::tz::TimeZone::UTC);
        tasks.push(EmailArchiveTask::task(
            archive.clone(),
            transport.clone(),
            &tz,
        )?);
        services.push(archive_recovery_service(archive.clone(), transport.clone()));
    }

    let subsystem = Subsystem {
        tasks,
        mcp_tools: tools,
        entities: entities(),
        managed_entities: managed_entities(),
        services,
        ..Subsystem::named("imap")
    };
    Ok((
        subsystem,
        ImapHandles {
            transport,
            archive,
            compose,
        },
    ))
}

/// After the transport's first successful start, one archive sweep recovers
/// claims a previous process left unresolved (TS runs it right after
/// `startEmailFeatures`).
fn archive_recovery_service(
    archive: ArchiveService,
    transport: ImapTransport,
) -> BackgroundService {
    BackgroundService {
        name: "imap-archive-recovery",
        start: Box::new(move |ctx: AppContext| {
            let archive = archive.clone();
            let transport = transport.clone();
            Box::pin(async move {
                let mut events = transport.mail_events();
                while !transport.is_active() {
                    tokio::select! {
                        () = ctx.shutdown.cancelled() => return,
                        received = events.recv() => {
                            if matches!(received, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                                return;
                            }
                        }
                    }
                }
                if let Err(error) = archive.sweep(&transport).await {
                    tracing::warn!(target: LOG, "Email archive recovery deferred to scheduled retry: {error}");
                }
            })
        }),
        retry: None,
    }
}
