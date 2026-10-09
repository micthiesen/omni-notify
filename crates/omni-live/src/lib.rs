//! Livestream monitoring (WP04): `channels.json`, aggregate streamer state
//! over platform bindings, edge notifications, viewer metrics and sessions,
//! Destiny.gg discovery with durable profile identity links, the
//! `LiveCheckTask`, the streamer routes and the `LiveDirectory` port.
//!
//! Wiring (WP14):
//! ```text
//! let live = omni_live::LiveModule::load(&ctx)?;              // fails boot on a bad channels.json
//! let ios = omni_ios_controls::IosControls::new(&ctx, live.roster()).await;
//! let live_subsystem = live.into_subsystem(Some(ios.reconciler()))?;   // sets ports.live_directory
//! let ios_subsystem = ios.into_subsystem();
//! ```

pub mod channels;
pub mod dgg;
pub mod directory;
pub mod display;
pub mod display_order;
pub mod error;
pub mod format;
pub mod identity;
pub mod metrics;
pub mod notification_policy;
pub mod notify;
pub mod outage;
pub mod platform;
pub mod platforms;
pub mod profile_links;
pub mod routes;
pub mod sessions;
pub mod status;
pub mod streamers;
pub mod task;
pub mod title_debounce;
pub mod transitions;
pub mod trigger_channels;

use std::collections::HashSet;
use std::sync::Arc;

use jiff::tz::TimeZone;
use omni_runtime::{AppContext, ManagedEntity, Subsystem};
use omni_store::entity::EntityDescriptor;

pub use channels::{ChannelsConfigError, LiveCheckConfig};
pub use directory::LiveDirectoryService;
pub use error::LiveError;
pub use platform::{Platform, PlatformBinding};
pub use streamers::{Roster, Streamer};
pub use task::{LiveCheck, LiveCheckTask, TickHook};

use crate::dgg::WebSocketDggFeed;
use crate::identity::ProfileIdentityLink;
use crate::metrics::{PlatformViewerMetrics, ViewerMetrics};
use crate::notify::PushoverNotifier;
use crate::platforms::{KickCredentials, Platforms};
use crate::profile_links::{ProfileFetcher, ProfileIdentityLearner};
use crate::sessions::StreamSessions;
use crate::status::StreamerStatus;
use crate::task::{DggDiscovery, LiveCheckDeps, PortIntelligence};

const LOG: &str = "Main";

/// Why the live subsystem cannot start (fails boot).
#[derive(Debug, thiserror::Error)]
pub enum LiveBootError {
    #[error(transparent)]
    Channels(#[from] ChannelsConfigError),
    #[error(transparent)]
    Streamers(#[from] streamers::BuildStreamersError),
    #[error("invalid TZ {tz:?}: {message}")]
    TimeZone { tz: String, message: String },
    #[error("invalid LiveCheckTask schedule: {0}")]
    Schedule(String),
}

/// Every entity this crate owns (for `migrate_all` and the compat audit).
pub fn entities() -> Vec<EntityDescriptor> {
    vec![
        EntityDescriptor::of::<StreamerStatus>(),
        EntityDescriptor::of::<StreamSessions>(),
        EntityDescriptor::of::<ViewerMetrics>(),
        EntityDescriptor::of::<PlatformViewerMetrics>(),
        EntityDescriptor::of::<ProfileIdentityLink>(),
    ]
}

/// Data manager rows, in `data-manager.ts` order.
pub fn managed_entities() -> Vec<ManagedEntity> {
    let managed = |slug, label, description, warning, entity, primary_key| ManagedEntity {
        slug,
        label,
        description,
        warning,
        entity,
        primary_key,
        can_delete: None,
        after_delete: None,
    };
    vec![
        managed(
            "streamer-status",
            "Streamer status",
            "Current aggregate live session and last offline state per streamer.",
            Some("Deleting live state can cause a fresh went-live transition on the next check."),
            EntityDescriptor::of::<StreamerStatus>(),
            &["streamerId"],
        ),
        managed(
            "streamer-sessions",
            "Stream sessions",
            "Completed live sessions per streamer (start, end, peak, title).",
            Some("Deleted session history cannot be reconstructed."),
            EntityDescriptor::of::<StreamSessions>(),
            &["streamerId"],
        ),
        managed(
            "streamer-viewer-metrics",
            "Viewer metrics",
            "Daily viewer peaks and all-time records per streamer.",
            Some("Deleted viewer records cannot be reconstructed outside the retained window."),
            EntityDescriptor::of::<ViewerMetrics>(),
            &["streamerId"],
        ),
        managed(
            "streamer-platform-viewer-metrics",
            "Streamer platform viewer metrics",
            "Per-platform daily viewer peaks for grouped streamer identities.",
            None,
            EntityDescriptor::of::<PlatformViewerMetrics>(),
            &["streamerId", "platform", "username"],
        ),
        managed(
            "live-profile-identity-link",
            "Livestream profile identity links",
            "Durable direct-profile associations between platform accounts.",
            None,
            EntityDescriptor::of::<ProfileIdentityLink>(),
            &["sourceBinding"],
        ),
    ]
}

/// The loaded live configuration, before the subsystem is assembled.
pub struct LiveModule {
    ctx: AppContext,
    roster: Roster,
    dgg_top_embeds: u32,
    tz: TimeZone,
    kick: Option<KickCredentials>,
}

impl LiveModule {
    /// Loads `CHANNELS_CONFIG_PATH` (default `./channels.json`). Any invalid
    /// configuration fails boot; a missing file means no streamers.
    pub fn load(ctx: &AppContext) -> Result<Self, LiveBootError> {
        let path = channels::config_path(ctx.config.channels_config_path.as_deref());
        let config = channels::load_channels_config(&path)?;
        Self::from_config(ctx, config)
    }

    /// Builds streamers from an already-parsed configuration. Kick bindings
    /// are dropped when Kick credentials are missing.
    pub fn from_config(ctx: &AppContext, config: LiveCheckConfig) -> Result<Self, LiveBootError> {
        let tz = TimeZone::get(&ctx.config.tz).map_err(|e| LiveBootError::TimeZone {
            tz: ctx.config.tz.clone(),
            message: e.to_string(),
        })?;
        let configured = streamers::build_streamers(&config.channels)?;
        let kick = match (&ctx.config.kick_client_id, &ctx.config.kick_client_secret) {
            (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => {
                Some(KickCredentials {
                    client_id: id.clone(),
                    client_secret: secret.clone(),
                })
            }
            _ => None,
        };
        let streamers = if kick.is_some() {
            configured
        } else {
            let all_ids: Vec<(String, String)> = configured
                .iter()
                .map(|s| (s.id.clone(), s.display_name.clone()))
                .collect();
            let (remaining, dropped_any) =
                streamers::drop_platform_bindings(configured, Platform::Kick);
            if dropped_any {
                tracing::warn!(
                    target: LOG,
                    "Kick channels configured but KICK_CLIENT_ID/KICK_CLIENT_SECRET missing; skipping Kick"
                );
                let fully_dropped: Vec<String> = all_ids
                    .into_iter()
                    .filter(|(id, _)| !remaining.iter().any(|s| &s.id == id))
                    .map(|(_, name)| name)
                    .collect();
                if !fully_dropped.is_empty() {
                    tracing::warn!(target: LOG, "Not tracked at all (Kick-only): {}", fully_dropped.join(", "));
                }
            }
            remaining
        };
        Ok(Self {
            ctx: ctx.clone(),
            roster: Roster::new(streamers),
            dgg_top_embeds: config.dgg_top_embeds,
            tz,
            kick,
        })
    }

    /// The shared streamer list (iOS controls read it).
    pub fn roster(&self) -> Roster {
        self.roster.clone()
    }

    pub fn time_zone(&self) -> &TimeZone {
        &self.tz
    }

    pub fn directory(&self) -> Arc<LiveDirectoryService> {
        Arc::new(LiveDirectoryService::new(
            self.ctx.store.clone(),
            self.roster.clone(),
        ))
    }

    /// The production live check over this configuration.
    pub fn live_check(&self, reconcile: Option<Arc<dyn TickHook>>) -> LiveCheck {
        let ctx = &self.ctx;
        let platforms = Platforms::new(ctx.public_http.clone(), self.kick.clone())
            .with_clock(ctx.clock.clone());
        let fetcher = Arc::new(platforms);
        let deps = LiveCheckDeps {
            store: ctx.store.clone(),
            clock: ctx.clock.clone(),
            tz: self.tz.clone(),
            fetcher,
            notifier: Arc::new(PushoverNotifier::new(ctx.pushover.clone())),
            offline_notifications: ctx.config.offline_notifications,
            live_token: ctx
                .config
                .pushover_token(omni_config::PushoverChannel::Live)
                .map(str::to_owned),
            bus: Some(ctx.bus.clone()),
        };
        let mut check = LiveCheck::new(self.roster.clone(), deps)
            .with_intelligence(Arc::new(PortIntelligence::new(ctx.ports.clone())));
        if self.dgg_top_embeds > 0 {
            let learner = ProfileIdentityLearner::new(
                ctx.store.clone(),
                ProfileFetcher::new(ctx.http.clone()),
            );
            check = check.with_dgg(DggDiscovery {
                top_embeds: self.dgg_top_embeds as usize,
                available_platforms: Platform::ALL.into_iter().collect::<HashSet<_>>(),
                feed: Arc::new(WebSocketDggFeed::default()),
                identity: Arc::new(learner),
            });
        }
        if let Some(hook) = reconcile {
            check = check.with_reconcile(hook);
        }
        check
    }

    /// Routes, the `LiveCheckTask` (when anything is tracked), entities, and
    /// the `LiveDirectory` port.
    pub fn into_subsystem(
        self,
        reconcile: Option<Arc<dyn TickHook>>,
    ) -> Result<Subsystem, LiveBootError> {
        if self.ctx.ports.set_live_directory(self.directory()).is_err() {
            tracing::warn!(target: LOG, "LiveDirectory port was already set; keeping the existing one");
        }
        let mut subsystem = Subsystem::named("live");
        if !self.roster.is_empty() || self.dgg_top_embeds > 0 {
            let check = Arc::new(self.live_check(reconcile));
            let task = LiveCheckTask::new(check, &self.tz)
                .map_err(|e| LiveBootError::Schedule(e.to_string()))?;
            subsystem.tasks.push(Arc::new(task));
        }
        subsystem.router = routes::router(routes::LiveRoutesState {
            store: self.ctx.store.clone(),
            roster: self.roster.clone(),
        });
        subsystem.entities = entities();
        subsystem.managed_entities = managed_entities();
        Ok(subsystem)
    }
}
