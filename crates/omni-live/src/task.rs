//! `LiveCheckTask`: one tick polls every due streamer, decides
//! aggregate transitions and notifies only on aggregate edges.
//!
//! Ordering guarantees:
//! - went-live persists the observed edge before notifying, so a delivery
//!   failure never makes the next tick rediscover and resend it;
//! - went-offline notifies before recording the session and writing the
//!   offline state, so a failed alert is retried and sessions are never
//!   appended twice;
//! - a pending viewer peak is flushed (confirmed) when the stream goes offline.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::StreamExt;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use jiff::tz::TimeZone;
use omni_api::events::LivestreamTransition;
use omni_api::streamers::StreamerTier;
use omni_core::clock::SharedClock;
use omni_runtime::ports::{self, LiveIntelligence, LiveTransition, Ports};
use omni_store::Store;
use omni_tasks::{AppEvent, CronSchedule, EventBus, RunContext, Task, TaskError, TaskOptions};

use crate::config::TopEmbeds;
use crate::dgg::{
    DggFeed, DggFeedSource, ResolvedDggStreams, canonical_binding, resolve_dgg_streams,
};
use crate::error::LiveError;
use crate::format::{format_count, format_distance};
use crate::identity::{ProfileIdentityLink, all_links, forget_link};
use crate::metrics::{ViewerMetricsService, ViewerObservation, record_platform_viewer_count};
use crate::notification_policy::{
    NotificationPermissions, ViewerRecordScope, live_notifications_enabled,
    notification_permissions,
};
use crate::notify::{LiveMessage, LiveNotifier};
use crate::outage::{OutageAlerter, OutageKind, UNREACHABLE_TICK_THRESHOLD, UnknownStreak};
use crate::platform::{FetchedStatus, Platform, PlatformBinding};
use crate::platforms::StatusFetcher;
use crate::profile_links::{IdentityLearner, LearnInput};
use crate::sessions::record_completed_session;
use crate::status::{LiveStatus, OfflineStatus, StreamerStatus, get_status, upsert_status};
use crate::streamers::{DiscoverySource, Roster, Streamer, is_streamer_due};
use crate::title_debounce::{DebounceAction, TitleChangeDebouncer};
use crate::transitions::{BindingFetchResult, TickDecision, decide_transition};

const LOG: &str = "LiveCheckTask";
/// Task name (persisted in run history).
pub const TASK_NAME: &str = "LiveCheckTask";
/// Every 20 seconds.
pub const SCHEDULE: &str = "*/20 * * * * *";
const JITTER: std::time::Duration = std::time::Duration::from_secs(3);
const STREAMER_CONCURRENCY: usize = 6;
const BINDING_CONCURRENCY: usize = 4;
const IDENTITY_CONCURRENCY: usize = 4;
const PROFILE_IDENTITY_RETRY_MS: i64 = 24 * 60 * 60 * 1000;
const PROFILE_IDENTITY_FAILURE_RETRY_MS: i64 = 5 * 60 * 1000;
const PROFILE_IDENTITY_VERIFICATION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const MAX_STREAK_ERROR_UTF16: usize = 300;

/// The per-tick observation handed to livestream intelligence.
#[derive(Clone, Debug)]
pub struct LiveObservation {
    pub streamer: Streamer,
    pub status: LiveStatus,
    pub went_live: bool,
    pub title_changed: bool,
}

/// Livestream-intelligence hooks (`LivestreamIntelligenceObserver`).
pub trait IntelligenceObserver: Send + Sync {
    fn observe_live<'a>(&'a self, observation: &'a LiveObservation, now: i64) -> BoxFuture<'a, ()>;
    fn observe_offline<'a>(&'a self, streamer_id: &'a str, now: i64) -> BoxFuture<'a, ()>;
    fn after_tick(&self) -> BoxFuture<'_, ()>;
}

/// Forwards to the `LiveIntelligence` port when it is set: every live
/// observation (`observe_live`, with the streamer and status serialized as the
/// `LiveDirectory` DTOs), aggregate edges (`on_transition`) and `after_tick`.
#[derive(Clone, Default)]
pub struct PortIntelligence {
    ports: Ports,
}

impl PortIntelligence {
    pub fn new(ports: Ports) -> Self {
        Self { ports }
    }

    fn port(&self) -> Option<Arc<dyn LiveIntelligence>> {
        self.ports.live_intelligence()
    }
}

impl IntelligenceObserver for PortIntelligence {
    fn observe_live<'a>(&'a self, observation: &'a LiveObservation, now: i64) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(port) = self.port() else {
                return;
            };
            if observation.went_live {
                let transition = LiveTransition {
                    streamer_id: observation.streamer.id.clone(),
                    live: true,
                    at_ms: now,
                };
                port.on_transition(&transition).await;
            }
            let status = StreamerStatus::Live(observation.status.clone());
            let streamer = crate::display::livestream_summary(&observation.streamer, &status);
            let payload = match (
                serde_json::to_value(&streamer),
                serde_json::to_value(crate::display::status_view(&status)),
            ) {
                (Ok(streamer), Ok(status)) => ports::LiveObservation {
                    streamer,
                    status,
                    went_live: observation.went_live,
                    title_changed: observation.title_changed,
                    at_ms: now,
                },
                (Err(error), _) | (_, Err(error)) => {
                    tracing::error!(
                        target: LOG,
                        "Intelligence observation for {} not serialized: {error}",
                        observation.streamer.display_name
                    );
                    return;
                }
            };
            port.observe_live(&payload).await;
        })
    }

    fn observe_offline<'a>(&'a self, streamer_id: &'a str, now: i64) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(port) = self.port() {
                let transition = LiveTransition {
                    streamer_id: streamer_id.to_owned(),
                    live: false,
                    at_ms: now,
                };
                port.on_transition(&transition).await;
            }
        })
    }

    fn after_tick(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Some(port) = self.port() {
                port.after_tick().await;
            }
        })
    }
}

/// Runs after every successful tick (iOS control reconciliation). Failures
/// are logged and never fail the tick.
pub trait TickHook: Send + Sync {
    fn after_tick(&self) -> BoxFuture<'_, Result<(), String>>;
}

/// Destiny.gg discovery configuration and seams.
#[derive(Clone)]
pub struct DggDiscovery {
    /// Shared with the configuration; 0 disables discovery.
    pub top_embeds: TopEmbeds,
    pub available_platforms: HashSet<Platform>,
    pub feed: Arc<dyn DggFeedSource>,
    pub identity: Arc<dyn IdentityLearner>,
}

/// Everything a tick needs.
#[derive(Clone)]
pub struct LiveCheckDeps {
    pub store: Store,
    pub clock: SharedClock,
    pub tz: TimeZone,
    pub fetcher: Arc<dyn StatusFetcher>,
    pub notifier: Arc<dyn LiveNotifier>,
    /// `OFFLINE_NOTIFICATIONS`.
    pub offline_notifications: bool,
    /// `PUSHOVER_LIVE_TOKEN ?? PUSHOVER_TOKEN`.
    pub live_token: Option<String>,
    /// Dashboard live updates (`StreamersChanged` after every tick).
    pub bus: Option<EventBus>,
}

#[derive(Default)]
struct TickState {
    tick_count: u64,
    logged_streamers: bool,
    unknown_streaks: IndexMap<String, UnknownStreak>,
    outage: OutageAlerter,
    debouncer: TitleChangeDebouncer,
    profile_identity_retry_at: HashMap<String, i64>,
    dgg_statuses: HashMap<String, FetchedStatus>,
    /// Every streamer polled and not yet retired, by id. One that leaves the
    /// roster (deleted, or a discovery dropped by a configuration change) is
    /// retired on the next tick.
    polled: IndexMap<String, Streamer>,
}

/// The live check: shared by the scheduled task and tests.
pub struct LiveCheck {
    roster: Roster,
    /// Configured streamers; the configuration service replaces them.
    configured: Roster,
    deps: LiveCheckDeps,
    dgg: Option<DggDiscovery>,
    reconcile: Option<Arc<dyn TickHook>>,
    intelligence: Option<Arc<dyn IntelligenceObserver>>,
    /// MCP Events publishing (through the `EventPublisher` port).
    events: Option<Ports>,
    metrics: ViewerMetricsService,
    state: Mutex<TickState>,
}

impl LiveCheck {
    /// `roster` starts as (and its configured part stays) its current list.
    pub fn new(roster: Roster, deps: LiveCheckDeps) -> Self {
        let configured = Roster::new(roster.snapshot());
        Self::with_configured(roster, configured, deps)
    }

    /// A live check over a shared configured list (edited at runtime).
    pub fn with_configured(roster: Roster, configured: Roster, deps: LiveCheckDeps) -> Self {
        let metrics =
            ViewerMetricsService::new(deps.store.clone(), deps.tz.clone(), deps.notifier.clone());
        Self {
            roster,
            configured,
            deps,
            dgg: None,
            reconcile: None,
            intelligence: None,
            events: None,
            metrics,
            state: Mutex::default(),
        }
    }

    pub fn with_dgg(mut self, dgg: DggDiscovery) -> Self {
        self.dgg = Some(dgg);
        self
    }

    pub fn with_reconcile(mut self, hook: Arc<dyn TickHook>) -> Self {
        self.reconcile = Some(hook);
        self
    }

    pub fn with_intelligence(mut self, observer: Arc<dyn IntelligenceObserver>) -> Self {
        self.intelligence = Some(observer);
        self
    }

    /// Publishes `livestream.status_changed` through `ports`' event publisher.
    pub fn with_events(mut self, ports: Ports) -> Self {
        self.events = Some(ports);
        self
    }

    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    fn state(&self) -> MutexGuard<'_, TickState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn now(&self) -> i64 {
        self.deps.clock.now_ms()
    }

    /// One live-check run.
    pub async fn tick(&self) -> Result<(), LiveError> {
        let tick = {
            let mut state = self.state();
            if !state.logged_streamers {
                state.logged_streamers = true;
                drop(state);
                self.log_streamers();
                state = self.state();
            }
            let tick = state.tick_count;
            state.tick_count += 1;
            tick
        };
        let polled = self.poll(tick).await;
        // Dashboard live updates follow every tick, including a failed one
        // (some streamers may have changed before the failure).
        if let Some(bus) = &self.deps.bus {
            bus.emit_app(AppEvent::StreamersChanged);
        }
        polled?;
        if let Some(intelligence) = &self.intelligence {
            intelligence.after_tick().await;
        }
        self.report_outage().await;
        if let Some(hook) = &self.reconcile
            && let Err(error) = hook.after_tick().await
        {
            // Controls are a convenience surface: never fail a status tick.
            tracing::warn!(target: LOG, "Failed to reconcile iOS controls: {error}");
        }
        Ok(())
    }

    /// The DGG refresh (on the background cadence) and every due streamer.
    /// All due streamers finish before the first failure is returned.
    async fn poll(&self, tick: u64) -> Result<(), LiveError> {
        let retired = self.retire_removed().await;
        // Background streamers skip ticks entirely; the startup tick includes them.
        if let Some(dgg) = &self.dgg
            && is_streamer_due(StreamerTier::Background, tick)
        {
            self.refresh_dgg(dgg).await?;
        }
        let due: Vec<Streamer> = self
            .roster
            .snapshot()
            .into_iter()
            .filter(|s| is_streamer_due(s.tier, tick))
            .collect();
        {
            let mut state = self.state();
            for streamer in &due {
                state.polled.insert(streamer.id.clone(), streamer.clone());
            }
        }
        let ticks: Vec<_> = due
            .iter()
            .map(|streamer| self.tick_streamer(streamer))
            .collect();
        let outcomes: Vec<Result<(), LiveError>> = futures::stream::iter(ticks)
            .buffer_unordered(STREAMER_CONCURRENCY)
            .collect()
            .await;
        retired.and(outcomes.into_iter().collect())
    }

    fn log_streamers(&self) {
        for streamer in self.roster.snapshot() {
            let bindings = streamer
                .bindings
                .iter()
                .map(|b| format!("{}:{}", b.platform, b.username))
                .collect::<Vec<_>>()
                .join(", ");
            let marker = if streamer.tier == StreamerTier::Background {
                " (background)"
            } else if live_notifications_enabled(streamer.live_notifications, streamer.tier) {
                ""
            } else {
                " (live notifications off)"
            };
            tracing::info!(target: LOG, "Streamer \"{}\" → {bindings}{marker}", streamer.display_name);
        }
    }

    // --- DGG ----------------------------------------------------------------

    async fn refresh_dgg(&self, dgg: &DggDiscovery) -> Result<(), LiveError> {
        if dgg.top_embeds.get() == 0 {
            return self.clear_dgg().await;
        }
        let feed = match dgg.feed.fetch().await {
            Ok(feed) => feed,
            Err(error) => {
                let message = format!("fetch DGG feed failed: {error}");
                tracing::warn!(target: LOG, "Failed to refresh Destiny.gg embeds: {message}");
                omni_tasks::report_degraded(message.clone());
                let mut state = self.state();
                for status in state.dgg_statuses.values_mut() {
                    *status = FetchedStatus::unknown(message.clone());
                }
                return Ok(());
            }
        };
        let mut links = all_links(&self.deps.store).await?;
        let mut resolution = self.resolve(&feed, dgg, &links);
        let configured_bindings: Vec<PlatformBinding> = self
            .configured
            .snapshot()
            .iter()
            .flat_map(|s| s.bindings.iter().cloned())
            .collect();
        let now = self.now();
        let links_by_source: HashMap<&str, &ProfileIdentityLink> = links
            .iter()
            .map(|l| (l.source_binding.as_str(), l))
            .collect();
        let mut candidates: Vec<(PlatformBinding, bool)> = resolution
            .discovered
            .iter()
            .filter_map(|entry| entry.streamer.bindings.first().cloned())
            .map(|binding| (binding, false))
            .collect();
        for entry in resolution.configured_sources.values().flatten() {
            let Some(source) = entry.streamer.bindings.first() else {
                continue;
            };
            let key = canonical_binding(source.platform, &source.username);
            if links_by_source
                .get(key.as_str())
                .is_some_and(|link| now - link.verified_at >= PROFILE_IDENTITY_VERIFICATION_MS)
            {
                candidates.push((source.clone(), true));
            }
        }

        let lookups: Vec<_> = candidates
            .into_iter()
            .map(|(source, verify)| {
                self.learn_identity(dgg, source, configured_bindings.clone(), verify, now)
            })
            .collect();
        let changes: Vec<Result<bool, LiveError>> = futures::stream::iter(lookups)
            .buffer_unordered(IDENTITY_CONCURRENCY)
            .collect()
            .await;
        let mut identity_changed = false;
        for change in changes {
            identity_changed |= change?;
        }
        if identity_changed {
            links = all_links(&self.deps.store).await?;
            resolution = self.resolve(&feed, dgg, &links);
        }

        let next_ids: HashSet<&str> = resolution
            .discovered
            .iter()
            .map(|entry| entry.streamer.id.as_str())
            .collect();
        let removed: Vec<Streamer> = self
            .roster
            .snapshot()
            .into_iter()
            .filter(|s| {
                s.discovery_source == Some(DiscoverySource::Dgg)
                    && !next_ids.contains(s.id.as_str())
            })
            .collect();
        for streamer in &removed {
            self.retire(streamer).await?;
        }

        {
            let mut state = self.state();
            state.dgg_statuses.clear();
            let selected = resolution.discovered.iter();
            let linked = resolution.configured_sources.values().flatten();
            for entry in selected.chain(linked) {
                if let Some(binding) = entry.streamer.bindings.first() {
                    state.dgg_statuses.insert(
                        canonical_binding(binding.platform, &binding.username),
                        FetchedStatus::Live(entry.status.clone()),
                    );
                }
            }
        }

        let enriched: Vec<Streamer> = self
            .configured
            .snapshot()
            .iter()
            .map(|streamer| enrich_configured(streamer, &resolution, &links))
            .collect();
        let summary = resolution
            .discovered
            .iter()
            .map(|entry| {
                let binding = entry.streamer.bindings.first();
                format!(
                    "{}:{}{}",
                    binding.map(|b| b.platform.as_str()).unwrap_or_default(),
                    binding.map(|b| b.username.as_str()).unwrap_or_default(),
                    if entry.streamer.dgg.is_some_and(|d| d.hosted) {
                        " (hosted)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let selected_count = resolution.discovered.len();
        let mut next = enriched;
        next.extend(
            resolution
                .discovered
                .into_iter()
                .map(|entry| entry.streamer),
        );
        self.roster.replace(next);
        tracing::debug!(
            target: LOG,
            "Destiny.gg discovery selected {selected_count}/{}{}",
            dgg.top_embeds.get(),
            if summary.is_empty() { String::new() } else { format!(": {summary}") }
        );
        Ok(())
    }

    /// Discovery is off: retire discoveries and drop DGG presence.
    async fn clear_dgg(&self) -> Result<(), LiveError> {
        let roster = self.roster.snapshot();
        if roster
            .iter()
            .all(|s| s.discovery_source.is_none() && s.dgg.is_none())
        {
            return Ok(());
        }
        for streamer in roster.iter().filter(|s| s.discovery_source.is_some()) {
            self.retire(streamer).await?;
        }
        self.state().dgg_statuses.clear();
        self.roster.replace(self.configured.snapshot());
        Ok(())
    }

    fn resolve(
        &self,
        feed: &DggFeed,
        dgg: &DggDiscovery,
        links: &[ProfileIdentityLink],
    ) -> ResolvedDggStreams {
        let aliases: HashMap<String, String> = links
            .iter()
            .map(|l| (l.source_binding.clone(), l.target_binding.clone()))
            .collect();
        resolve_dgg_streams(
            feed,
            dgg.top_embeds.get() as usize,
            &self.configured.snapshot(),
            &dgg.available_platforms,
            &aliases,
        )
    }

    async fn learn_identity(
        &self,
        dgg: &DggDiscovery,
        source: PlatformBinding,
        configured_bindings: Vec<PlatformBinding>,
        verify: bool,
        now: i64,
    ) -> Result<bool, LiveError> {
        let key = canonical_binding(source.platform, &source.username);
        {
            let mut state = self.state();
            if state
                .profile_identity_retry_at
                .get(&key)
                .is_some_and(|retry_at| now < *retry_at)
            {
                return Ok(false);
            }
            state
                .profile_identity_retry_at
                .insert(key.clone(), now + PROFILE_IDENTITY_RETRY_MS);
        }
        let outcome = dgg
            .identity
            .learn(LearnInput {
                source: source.clone(),
                configured_bindings,
                now,
                force_refresh: verify,
            })
            .await;
        let (link, lookup_failed) = match outcome {
            Ok(link) => (link, false),
            Err(error) => {
                self.state()
                    .profile_identity_retry_at
                    .insert(key, now + PROFILE_IDENTITY_FAILURE_RETRY_MS);
                tracing::debug!(
                    target: LOG,
                    "Could not resolve profile identity for {}:{}: {error}",
                    source.platform,
                    source.username
                );
                (None, true)
            }
        };
        // A confirmed no-match on revalidation removes the alias; a failed
        // lookup keeps the last verified link.
        if verify && !lookup_failed && link.is_none() {
            forget_link(&self.deps.store, &source).await?;
            tracing::info!(
                target: LOG,
                "Removed stale profile identity for {}:{}",
                source.platform,
                source.username
            );
            return Ok(true);
        }
        Ok(link.is_some())
    }

    /// Leaving DGG's top set is not proof the stream ended: retire the
    /// status without a completed session or a confirmed viewer record.
    /// Configured streamers removed since the last tick: closed like a
    /// discovery that left the embeds (no offline notification).
    /// Retires polled streamers that left the roster. A failed retire stays
    /// pending for the next tick and does not stop this one.
    async fn retire_removed(&self) -> Result<(), LiveError> {
        let current: HashSet<String> = self.roster.snapshot().into_iter().map(|s| s.id).collect();
        let removed: Vec<Streamer> = self
            .state()
            .polled
            .values()
            .filter(|s| !current.contains(&s.id))
            .cloned()
            .collect();
        let mut result = Ok(());
        for streamer in &removed {
            match self.retire(streamer).await {
                Ok(()) => {
                    self.state().polled.shift_remove(&streamer.id);
                }
                Err(error) => {
                    if result.is_ok() {
                        result = Err(error);
                    }
                }
            }
        }
        result
    }

    async fn retire(&self, streamer: &Streamer) -> Result<(), LiveError> {
        if let StreamerStatus::Live(previous) = get_status(&self.deps.store, &streamer.id).await? {
            let mut offline = OfflineStatus::never_live(streamer.id.clone());
            offline.last_ended_at = Some(self.now());
            offline.last_started_at = Some(previous.started_at);
            offline.last_max_viewer_count = Some(previous.max_viewer_count);
            upsert_status(&self.deps.store, StreamerStatus::Offline(offline)).await?;
        }
        {
            let mut state = self.state();
            state.unknown_streaks.shift_remove(&streamer.id);
            state.debouncer.clear(&streamer.id);
            state.polled.shift_remove(&streamer.id);
            for binding in &streamer.bindings {
                state
                    .dgg_statuses
                    .remove(&canonical_binding(binding.platform, &binding.username));
            }
        }
        self.metrics.discard_pending_peaks(&streamer.id);
        if let Some(intelligence) = &self.intelligence {
            intelligence.observe_offline(&streamer.id, self.now()).await;
        }
        Ok(())
    }

    // --- outage -----------------------------------------------------------------

    /// One alert for the whole fleet, sent directly: the alerter owns the
    /// cadence, so the log line stays below ERROR to avoid a second alert.
    async fn report_outage(&self) {
        let total = self.roster.len();
        let now = self.now();
        let alert = {
            let mut state = self.state();
            let streaks: Vec<UnknownStreak> = state.unknown_streaks.values().cloned().collect();
            state.outage.evaluate(&streaks, total, now)
        };
        let Some(alert) = alert else {
            return;
        };
        let line = format!("{}\n{}", alert.title, alert.message);
        match alert.kind {
            OutageKind::Degraded => tracing::warn!(target: LOG, "{line}"),
            OutageKind::Recovered => tracing::info!(target: LOG, "{line}"),
        }
        let message = LiveMessage {
            title: alert.title,
            message: alert.message,
            url: None,
        };
        if let Err(error) = self.deps.notifier.send(None, message).await {
            // The outage state already advanced; this exact alert cannot be retried.
            tracing::error!(target: LOG, "Failed to send live-check outage notification: {error}");
        }
    }

    // --- per streamer -------------------------------------------------------------

    async fn tick_streamer(&self, streamer: &Streamer) -> Result<(), LiveError> {
        let fetches: Vec<_> = streamer
            .bindings
            .iter()
            .map(|binding| self.fetch_binding(binding))
            .collect();
        let results: Vec<BindingFetchResult> = futures::stream::iter(fetches)
            .buffered(BINDING_CONCURRENCY)
            .collect()
            .await;
        for result in &results {
            log_binding_status(&streamer.display_name, result);
        }

        let previous = get_status(&self.deps.store, &streamer.id).await?;
        let now = self.now();
        let decision = decide_transition(&streamer.id, &previous, &results, now);
        if let TickDecision::AllUnknown { errors } = &decision {
            self.handle_all_unknown(streamer, errors);
            return Ok(());
        }
        self.clear_unknown_streak(streamer);
        match decision {
            TickDecision::AllUnknown { .. } | TickDecision::NoChange => Ok(()),
            TickDecision::WentLive {
                next,
                summed_viewer_count,
            } => {
                self.handle_went_live(streamer, &previous, next, summed_viewer_count)
                    .await
            }
            TickDecision::WentOffline {
                previous_live,
                next,
            } => {
                self.handle_went_offline(streamer, &previous_live, next)
                    .await
            }
            TickDecision::StillLive {
                next,
                summed_viewer_count,
                title_changed,
                primary_switched,
            } => {
                self.handle_still_live(
                    streamer,
                    next,
                    summed_viewer_count,
                    title_changed,
                    primary_switched,
                )
                .await
            }
        }
    }

    async fn fetch_binding(&self, binding: &PlatformBinding) -> BindingFetchResult {
        let discovered = self
            .state()
            .dgg_statuses
            .get(&canonical_binding(binding.platform, &binding.username))
            .cloned();
        let status = match discovered {
            Some(status) => status,
            None => self.deps.fetcher.fetch(binding).await,
        };
        BindingFetchResult {
            binding: binding.clone(),
            status,
        }
    }

    /// Records the streak only; notifying is the outage alerter's job.
    fn handle_all_unknown(&self, streamer: &Streamer, errors: &[String]) {
        let error = errors
            .iter()
            .filter(|e| !e.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let error = omni_core::js::utf16_slice(&error, 0, MAX_STREAK_ERROR_UTF16).into_owned();
        let ticks = {
            let mut state = self.state();
            let ticks = state
                .unknown_streaks
                .get(&streamer.id)
                .map_or(0, |s| s.ticks)
                + 1;
            state.unknown_streaks.insert(
                streamer.id.clone(),
                UnknownStreak {
                    display_name: streamer.display_name.clone(),
                    ticks,
                    error: error.clone(),
                },
            );
            ticks
        };
        let message = format!(
            "{}: {ticks} consecutive all-unknown ticks: {error}",
            streamer.display_name
        );
        if ticks == UNREACHABLE_TICK_THRESHOLD {
            tracing::info!(target: LOG, "{message}");
        } else {
            tracing::debug!(target: LOG, "{message}");
        }
    }

    fn clear_unknown_streak(&self, streamer: &Streamer) {
        let streak = self.state().unknown_streaks.shift_remove(&streamer.id);
        if let Some(streak) = streak
            && streak.ticks >= UNREACHABLE_TICK_THRESHOLD
        {
            tracing::info!(
                target: LOG,
                "{} reachable again after {} all-unknown ticks",
                streamer.display_name,
                streak.ticks
            );
        }
    }

    async fn handle_went_live(
        &self,
        streamer: &Streamer,
        previous: &StreamerStatus,
        next: LiveStatus,
        summed_viewer_count: i64,
    ) -> Result<(), LiveError> {
        let now = self.now();
        tracing::info!(
            target: LOG,
            "{} is now LIVE (primary {}:{})",
            streamer.display_name,
            next.primary.platform,
            next.primary.username
        );
        // The go-live notification carries the title, so it is the
        // debouncer's baseline.
        self.state()
            .debouncer
            .seed(&streamer.id, &next.primary_title, now);
        // Persist the edge before notifying.
        upsert_status(&self.deps.store, StreamerStatus::Live(next.clone())).await?;
        crate::events::publish(
            self.events.as_ref(),
            crate::events::status_changed(
                streamer,
                &next,
                LivestreamTransition::WentLive,
                None,
                now,
            ),
        )
        .await;

        if self.permissions(streamer).went_live {
            let message = LiveMessage {
                title: format!("{} is LIVE!", streamer.display_name),
                message: build_live_message(&next.primary_title, previous, now, &self.deps.tz),
                url: Some(next.primary.notification_url_fields()),
            };
            self.deps
                .notifier
                .send(self.token_for(streamer).as_deref(), message)
                .await?;
        }

        self.record_viewers_if_any(streamer, &next, summed_viewer_count)
            .await?;
        if let Some(intelligence) = &self.intelligence {
            let observation = LiveObservation {
                streamer: streamer.clone(),
                status: next,
                went_live: true,
                title_changed: false,
            };
            intelligence.observe_live(&observation, now).await;
        }
        Ok(())
    }

    async fn handle_still_live(
        &self,
        streamer: &Streamer,
        next: LiveStatus,
        summed_viewer_count: i64,
        title_changed: bool,
        primary_switched: bool,
    ) -> Result<(), LiveError> {
        let now = self.now();
        if primary_switched {
            tracing::info!(
                target: LOG,
                "{} primary switched to {}:{}",
                streamer.display_name,
                next.primary.platform,
                next.primary.username
            );
            // A title held from the old primary must not fire under the new one.
            self.state().debouncer.clear(&streamer.id);
        }
        if title_changed {
            tracing::info!(target: LOG, "{} changed title", streamer.display_name);
        }
        // The observation is authoritative even if a notification fails.
        upsert_status(&self.deps.store, StreamerStatus::Live(next.clone())).await?;

        // Observed on every still-live tick so a held title can fire later.
        if self.permissions(streamer).title_change {
            let action = self.state().debouncer.observe(
                &streamer.id,
                &next.primary_title,
                title_changed,
                now,
            );
            if let DebounceAction::Notify { title } = action {
                let message = LiveMessage {
                    title: format!("{} changed title", streamer.display_name),
                    message: title,
                    url: Some(next.primary.notification_url_fields()),
                };
                self.deps
                    .notifier
                    .send(self.token_for(streamer).as_deref(), message)
                    .await?;
            }
        }

        self.record_viewers_if_any(streamer, &next, summed_viewer_count)
            .await?;
        if let Some(intelligence) = &self.intelligence {
            let observation = LiveObservation {
                streamer: streamer.clone(),
                status: next,
                went_live: false,
                title_changed,
            };
            intelligence.observe_live(&observation, now).await;
        }
        Ok(())
    }

    async fn handle_went_offline(
        &self,
        streamer: &Streamer,
        previous_live: &LiveStatus,
        next: OfflineStatus,
    ) -> Result<(), LiveError> {
        let now = self.now();
        tracing::info!(target: LOG, "{} is now offline", streamer.display_name);
        self.state().debouncer.clear(&streamer.id);

        let observation = ViewerObservation {
            streamer_id: streamer.id.clone(),
            display_name: streamer.display_name.clone(),
            viewer_count: 0,
            url_fields: previous_live.primary.notification_url_fields(),
            token: self.token_for(streamer),
            scope: self.record_scope(streamer),
        };
        self.metrics.flush_pending_peaks(&observation, now).await?;

        if self.permissions(streamer).went_offline {
            let duration = format_distance(now, previous_live.started_at, &self.deps.tz);
            let base = format!("Streamed for {duration}");
            let message = if previous_live.max_viewer_count > 0 {
                format!(
                    "{base} with {}.",
                    format_count(previous_live.max_viewer_count)
                )
            } else {
                format!("{base}.")
            };
            let message = LiveMessage {
                title: format!("{} is now offline", streamer.display_name),
                message,
                url: None,
            };
            self.deps
                .notifier
                .send(self.token_for(streamer).as_deref(), message)
                .await?;
        }

        // The live state stays the retry marker until the alert is delivered;
        // the session is closed only afterwards so retries cannot duplicate it.
        // The event is published before that write for the same reason: a
        // replay keeps its dedup key.
        let ended_at = next.last_ended_at.unwrap_or(now);
        crate::events::publish(
            self.events.as_ref(),
            crate::events::status_changed(
                streamer,
                previous_live,
                LivestreamTransition::WentOffline,
                Some(ended_at),
                now,
            ),
        )
        .await;
        record_completed_session(&self.deps.store, previous_live, ended_at).await?;
        upsert_status(&self.deps.store, StreamerStatus::Offline(next)).await?;

        if let Some(intelligence) = &self.intelligence {
            intelligence.observe_offline(&streamer.id, now).await;
        }
        Ok(())
    }

    async fn record_viewers_if_any(
        &self,
        streamer: &Streamer,
        status: &LiveStatus,
        summed_viewer_count: i64,
    ) -> Result<(), LiveError> {
        let now = self.now();
        if summed_viewer_count > 0 {
            let observation = ViewerObservation {
                streamer_id: streamer.id.clone(),
                display_name: streamer.display_name.clone(),
                viewer_count: summed_viewer_count,
                url_fields: status.primary.notification_url_fields(),
                token: self.token_for(streamer),
                scope: self.record_scope(streamer),
            };
            self.metrics.record_viewer_count(&observation, now).await?;
        }
        for source in status.sources.iter().flatten() {
            let Some(count) = source.viewer_count.filter(|c| *c > 0) else {
                continue;
            };
            record_platform_viewer_count(
                &self.deps.store,
                &streamer.id,
                source.platform,
                &source.username,
                count,
                now,
                &self.deps.tz,
            )
            .await?;
        }
        Ok(())
    }

    fn current(&self, streamer: &Streamer) -> Streamer {
        self.roster
            .get(&streamer.id)
            .unwrap_or_else(|| streamer.clone())
    }

    fn permissions(&self, streamer: &Streamer) -> NotificationPermissions {
        notification_permissions(
            streamer.live_notifications,
            streamer.tier,
            self.deps.offline_notifications,
        )
    }

    fn record_scope(&self, streamer: &Streamer) -> ViewerRecordScope {
        match self.roster.get(&streamer.id) {
            Some(current) => self.permissions(&current).viewer_records,
            None => ViewerRecordScope::All,
        }
    }

    /// The streamer's own token, else the live channel token.
    fn token_for(&self, streamer: &Streamer) -> Option<String> {
        self.current(streamer)
            .pushover_token
            .or_else(|| self.deps.live_token.clone())
    }
}

/// Configured streamer plus its DGG presence and alias-linked sources (not
/// duplicating an account it already polls).
fn enrich_configured(
    streamer: &Streamer,
    resolution: &ResolvedDggStreams,
    links: &[ProfileIdentityLink],
) -> Streamer {
    let presence = resolution.configured_presence.get(&streamer.id).copied();
    let existing_keys: Vec<(Platform, String)> = streamer
        .bindings
        .iter()
        .map(|b| (b.platform, canonical_binding(b.platform, &b.username)))
        .collect();
    let linked: Vec<PlatformBinding> = resolution
        .configured_sources
        .get(&streamer.id)
        .into_iter()
        .flatten()
        .flat_map(|entry| entry.streamer.bindings.iter().cloned())
        .filter(|binding| {
            let key = canonical_binding(binding.platform, &binding.username);
            // An alias to an existing account on the same platform is the same
            // audience (a YouTube video and its owner): keep it for identity
            // revalidation but never poll or count it twice.
            let aliased_to_existing = links.iter().any(|link| {
                link.source_binding == key
                    && existing_keys.iter().any(|(platform, existing)| {
                        *platform == binding.platform && *existing == link.target_binding
                    })
            });
            let already_bound = existing_keys.iter().any(|(_, existing)| *existing == key);
            !aliased_to_existing && !already_bound
        })
        .collect();
    if presence.is_none() && linked.is_empty() {
        return streamer.clone();
    }
    let mut enriched = streamer.clone();
    enriched.dgg = presence;
    enriched.bindings.extend(linked);
    enriched
}

fn log_binding_status(display_name: &str, result: &BindingFetchResult) {
    let place = format!(
        "{display_name} [{}:{}]",
        result.binding.platform, result.binding.username
    );
    match &result.status {
        FetchedStatus::Live(live) => {
            tracing::debug!(target: LOG, "{place} is live: \"{}\"", live.title)
        }
        FetchedStatus::Offline => tracing::debug!(target: LOG, "{place} is offline"),
        FetchedStatus::Unknown { error } => {
            tracing::debug!(target: LOG, "{place} unknown: {error}")
        }
    }
}

/// The go-live message, with the previous session when known.
pub fn build_live_message(
    primary_title: &str,
    previous: &StreamerStatus,
    now: i64,
    tz: &TimeZone,
) -> String {
    let StreamerStatus::Offline(previous) = previous else {
        return primary_title.to_owned();
    };
    let (Some(ended), Some(started)) = (previous.last_ended_at, previous.last_started_at) else {
        return primary_title.to_owned();
    };
    let ago = format_distance(ended, now, tz);
    let duration = format_distance(ended, started, tz);
    let suffix = match previous.last_max_viewer_count.filter(|count| *count != 0) {
        Some(count) => format!(
            "Last live {ago} ago for {duration} with {}.",
            format_count(count)
        ),
        None => format!("Last live {ago} ago for {duration}."),
    };
    format!("{primary_title}\n\n{suffix}")
}

/// The scheduled task over a [`LiveCheck`].
pub struct LiveCheckTask {
    check: Arc<LiveCheck>,
    schedule: CronSchedule,
}

impl LiveCheckTask {
    pub fn new(
        check: Arc<LiveCheck>,
        tz: &TimeZone,
    ) -> Result<Self, omni_tasks::InvalidScheduleError> {
        Ok(Self {
            check,
            schedule: CronSchedule::parse(SCHEDULE, tz)?,
        })
    }
}

impl Task for LiveCheckTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions {
            jitter: JITTER,
            run_on_startup: true,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move { self.check.tick().await.map_err(TaskError::from_error) })
    }
}
