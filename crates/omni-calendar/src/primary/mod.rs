//! Management of Michael's one primary iCloud calendar: identity pinning, a
//! local mirror kept by `sync-collection` (with an ETag fallback), a durable
//! change feed, and idempotent, verified writes for the MCP tools.

pub mod client;
pub mod edit;
pub mod events;
pub mod expand;
pub mod ics;
pub mod identity;
pub mod model;
pub mod operations;
pub mod rrule;
pub mod starting;
pub mod store;
pub mod sync;
pub mod task;
pub mod time;

use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_http::{HttpClient, SideEffectMode};
use omni_runtime::ports::Ports;
use omni_store::entity::EntityOps as _;
use omni_store::{Store, StoreError};
use tokio_util::task::TaskTracker;

use crate::caldav::{CaldavSettings, RecordedCaldavWrite};
use client::DavClient;
use expand::Occurrence;
use ics::IcsDoc;
use identity::PrimaryIdentity;
use store::{ChangeRow, MirrorResource, PrimaryPin, SINGLETON, SyncState};
use sync::{SyncCtx, SyncReport};
use time::Zones;

const LOG: &str = "CalendarPrimary";

/// How long a resolved identity is reused.
pub const IDENTITY_TTL_MS: i64 = 6 * 60 * 60 * 1000;
/// Reads sync first when the mirror is older than this.
pub const READ_MAX_AGE_MS: i64 = 30 * 1000;
/// The background task syncs when the mirror is older than this.
pub const BACKGROUND_MAX_AGE_MS: i64 = 5 * 60 * 1000;

/// A primary-calendar failure. `code` is a stable machine-readable reason.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PrimaryError {
    #[error("{message}")]
    Coded { code: &'static str, message: String },
    #[error("{message}")]
    Transport { message: String, transient: bool },
    #[error("calendar storage failed: {0}")]
    Store(String),
}

impl PrimaryError {
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            message: message.into(),
        }
    }

    pub fn identity(code: &'static str, message: impl Into<String>) -> Self {
        Self::coded(code, message)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::coded("caldav_protocol", message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::coded("invalid_input", message)
    }

    pub fn store(error: StoreError) -> Self {
        Self::Store(error.to_string())
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Coded { code, .. } => code,
            Self::Transport { .. } => "caldav_unavailable",
            Self::Store(_) => "storage_failed",
        }
    }

    /// `[code] message`, the text tool errors carry.
    pub fn tool_text(&self) -> String {
        format!("[{}] {self}", self.code())
    }
}

impl From<StoreError> for PrimaryError {
    fn from(error: StoreError) -> Self {
        Self::store(error)
    }
}

/// Everything the service needs.
pub struct PrimaryDeps {
    pub http: HttpClient,
    pub settings: Option<CaldavSettings>,
    pub store: Store,
    pub clock: SharedClock,
    pub mode: SideEffectMode,
    /// Record-mode captures (shared with the pipeline writer).
    pub recorded: Arc<Mutex<Vec<RecordedCaldavWrite>>>,
    pub default_tz: String,
    pub tracker: Option<TaskTracker>,
    /// Read at publish time for the MCP Events port (set after this service
    /// is built; unset when MCP Events are disabled).
    pub ports: Ports,
}

struct Inner {
    deps: PrimaryDeps,
    default_tz: TimeZone,
    identity: Mutex<Option<PrimaryIdentity>>,
    identity_error: Mutex<Option<PrimaryError>>,
    sync_lock: tokio::sync::Mutex<()>,
    write_lock: tokio::sync::Mutex<()>,
}

/// The primary calendar service (cheap to clone).
#[derive(Clone)]
pub struct PrimaryCalendar {
    inner: Arc<Inner>,
}

/// How fresh a read's data is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Freshness {
    pub synced_at: Option<i64>,
    /// The sync before this read failed; data is from `synced_at`.
    pub stale: bool,
    pub error: Option<String>,
}

/// One occurrence plus the resource it belongs to.
#[derive(Clone, Debug)]
pub struct FoundOccurrence {
    pub event_id: String,
    pub etag: String,
    pub occurrence: Occurrence,
}

/// Status for the tool, the API and the UI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub configured: bool,
    /// `ready`, `not_configured`, `identity_error` or `sync_error`.
    pub state: &'static str,
    pub message: Option<String>,
    pub error_code: Option<&'static str>,
    pub is_server_default: Option<bool>,
    pub pipeline_targets_primary: Option<bool>,
    pub supports_sync: Option<bool>,
    pub writable: Option<bool>,
    pub pinned_at: Option<i64>,
    pub repinned_at: Option<i64>,
    pub last_sync_at: Option<i64>,
    pub last_full_sync_at: Option<i64>,
    pub last_error: Option<String>,
    pub event_count: u64,
    pub change_cursor: i64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl PrimaryCalendar {
    pub fn new(deps: PrimaryDeps) -> Self {
        let default_tz = TimeZone::get(&deps.default_tz).unwrap_or(TimeZone::UTC);
        Self {
            inner: Arc::new(Inner {
                deps,
                default_tz,
                identity: Mutex::new(None),
                identity_error: Mutex::new(None),
                sync_lock: tokio::sync::Mutex::new(()),
                write_lock: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.inner.deps.settings.is_some()
    }

    pub fn default_tz(&self) -> &TimeZone {
        &self.inner.default_tz
    }

    pub fn default_tz_name(&self) -> &str {
        &self.inner.deps.default_tz
    }

    pub fn store(&self) -> &Store {
        &self.inner.deps.store
    }

    pub fn now_ms(&self) -> i64 {
        self.inner.deps.clock.now_ms()
    }

    pub fn now(&self) -> Timestamp {
        Timestamp::from_millisecond(self.now_ms()).unwrap_or(Timestamp::UNIX_EPOCH)
    }

    pub(crate) fn tracker(&self) -> Option<&TaskTracker> {
        self.inner.deps.tracker.as_ref()
    }

    pub(crate) fn write_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.inner.write_lock
    }

    /// Serializes syncs; a write holds it from send until its echo is
    /// recorded, so a sync never classifies Omni's own write as external.
    pub(crate) fn sync_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.inner.sync_lock
    }

    pub fn ports(&self) -> &Ports {
        &self.inner.deps.ports
    }

    /// The resolved identity, reusing a cached one for [`IDENTITY_TTL_MS`].
    pub async fn identity(&self, force: bool) -> Result<PrimaryIdentity, PrimaryError> {
        let Some(settings) = &self.inner.deps.settings else {
            return Err(PrimaryError::coded(
                "not_configured",
                "no iCloud CalDAV credentials are configured",
            ));
        };
        let now = self.now_ms();
        if !force
            && let Some(cached) = lock(&self.inner.identity).clone()
            && now - cached.resolved_at < IDENTITY_TTL_MS
        {
            return Ok(cached);
        }
        match identity::resolve_primary(&self.inner.deps.http, settings, self.store(), now).await {
            Ok(resolved) => {
                *lock(&self.inner.identity) = Some(resolved.clone());
                *lock(&self.inner.identity_error) = None;
                Ok(resolved)
            }
            Err(error) => {
                tracing::warn!(target: LOG, "Primary calendar unavailable: {error}");
                *lock(&self.inner.identity) = None;
                *lock(&self.inner.identity_error) = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Drops the cached identity (after 401/403/404 or a redirect).
    pub fn invalidate_identity(&self) {
        *lock(&self.inner.identity) = None;
    }

    pub(crate) fn client(&self, identity: &PrimaryIdentity) -> DavClient {
        DavClient::new(
            self.inner.deps.http.clone(),
            identity.auth_header.clone(),
            self.inner.deps.mode,
            self.inner.deps.recorded.clone(),
        )
    }

    async fn sync_state(&self) -> Result<Option<SyncState>, PrimaryError> {
        Ok(self
            .store()
            .read(|docs| docs.get::<SyncState>(&SINGLETON.to_owned()))
            .await?)
    }

    /// Syncs now (serialized with other syncs).
    pub async fn sync_now(&self) -> Result<SyncReport, PrimaryError> {
        let result = {
            let _guard = self.inner.sync_lock.lock().await;
            self.sync_locked().await
        };
        if result.is_ok() {
            self.publish_changes().await;
        }
        result
    }

    /// Hands new change rows to `calendar.event_changed`, outside the sync
    /// lock. Concurrent passes are safe: the outbox drops replayed dedup keys
    /// and the published cursor only moves forward. A failed pass retries
    /// after the next sync.
    async fn publish_changes(&self) {
        if let Err(error) = events::publish_changes(self).await {
            tracing::warn!(target: LOG, "Calendar change events not published: {error}");
        }
    }

    async fn sync_locked(&self) -> Result<SyncReport, PrimaryError> {
        let identity = self.identity(false).await?;
        let client = self.client(&identity);
        let now = self.now_ms();
        let ctx = SyncCtx {
            client: &client,
            identity: &identity,
            store: self.store(),
            now_ms: now,
            default_tz: &self.inner.default_tz,
        };
        let result = sync::sync(&ctx).await;
        if let Err(error) = &result {
            tracing::warn!(target: LOG, "Calendar sync failed: {error}");
            self.invalidate_identity();
            sync::record_failure(self.store(), now, error.to_string()).await?;
        }
        result
    }

    /// Syncs when the mirror is older than `max_age_ms` (or was never
    /// synced). A failed sync over an existing mirror serves it as stale.
    pub async fn ensure_fresh(&self, max_age_ms: i64) -> Result<Freshness, PrimaryError> {
        let fresh_enough = |state: &Option<SyncState>, now: i64| {
            state
                .as_ref()
                .and_then(|s| s.last_sync_at)
                .is_some_and(|at| now - at < max_age_ms)
        };
        let state = self.sync_state().await?;
        if fresh_enough(&state, self.now_ms()) {
            return Ok(Freshness {
                synced_at: state.and_then(|s| s.last_sync_at),
                ..Freshness::default()
            });
        }
        let guard = self.inner.sync_lock.lock().await;
        let state = self.sync_state().await?;
        if fresh_enough(&state, self.now_ms()) {
            return Ok(Freshness {
                synced_at: state.and_then(|s| s.last_sync_at),
                ..Freshness::default()
            });
        }
        let result = self.sync_locked().await;
        drop(guard);
        match result {
            Ok(_) => {
                self.publish_changes().await;
                Ok(Freshness {
                    synced_at: Some(self.now_ms()),
                    ..Freshness::default()
                })
            }
            Err(error) => {
                let synced_at = state.as_ref().and_then(|s| s.last_sync_at);
                let baselined = state.as_ref().is_some_and(|s| s.baselined);
                if baselined && matches!(error, PrimaryError::Transport { .. }) {
                    Ok(Freshness {
                        synced_at,
                        stale: true,
                        error: Some(error.to_string()),
                    })
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Every mirrored resource.
    pub async fn mirror(&self) -> Result<Vec<MirrorResource>, PrimaryError> {
        Ok(self
            .store()
            .read(|docs| docs.get_all::<MirrorResource>())
            .await?)
    }

    pub async fn mirror_row(&self, event_id: &str) -> Result<Option<MirrorResource>, PrimaryError> {
        let key = event_id.to_owned();
        Ok(self
            .store()
            .read(move |docs| docs.get::<MirrorResource>(&key))
            .await?)
    }

    /// Occurrences overlapping `[from, to)` across the mirror, sorted by
    /// start; also whether any series expansion was cut short.
    pub async fn occurrences(
        &self,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<(Vec<FoundOccurrence>, bool), PrimaryError> {
        let owner = lock(&self.inner.identity)
            .as_ref()
            .map(|i| i.owner_addresses.clone())
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut truncated = false;
        for row in self.mirror().await? {
            let Some(doc) = row.ics.as_deref().and_then(|t| IcsDoc::parse(t).ok()) else {
                continue;
            };
            let zones = Zones::new(&doc, self.inner.default_tz.clone());
            let expanded = expand::expand(&doc, &zones, &owner, from, to);
            truncated |= expanded.truncated;
            out.extend(
                expanded
                    .occurrences
                    .into_iter()
                    .map(|occurrence| FoundOccurrence {
                        event_id: row.event_id.clone(),
                        etag: row.etag.clone(),
                        occurrence,
                    }),
            );
        }
        out.sort_by(|a, b| {
            a.occurrence
                .start_utc
                .cmp(&b.occurrence.start_utc)
                .then_with(|| a.event_id.cmp(&b.event_id))
        });
        Ok((out, truncated))
    }

    /// The owner addresses of the cached identity (for scheduling roles).
    pub fn owner(&self) -> std::collections::BTreeSet<String> {
        lock(&self.inner.identity)
            .as_ref()
            .map(|i| i.owner_addresses.clone())
            .unwrap_or_default()
    }

    /// Change rows after `cursor` (exclusive), oldest first, at most `limit`,
    /// only those from `origin` when given; also the next cursor and whether
    /// more rows follow. Without more rows the cursor moves past every row
    /// scanned (filtered ones too); with none at all it is the input cursor,
    /// or the newest sequence when there is no input cursor.
    pub async fn changes_since(
        &self,
        cursor: Option<i64>,
        limit: usize,
        origin: Option<store::ChangeOrigin>,
    ) -> Result<(Vec<ChangeRow>, i64, bool), PrimaryError> {
        let (mut rows, state) = self
            .store()
            .read(|docs| {
                Ok((
                    docs.get_all::<ChangeRow>()?,
                    docs.get::<SyncState>(&SINGLETON.to_owned())?,
                ))
            })
            .await?;
        let latest = state.map_or(0, |s| s.change_seq);
        let after = cursor.unwrap_or(0);
        rows.retain(|r| r.seq > after);
        rows.sort_by_key(|r| r.seq);
        let scanned_to = rows.last().map(|r| r.seq);
        let mut matching = rows
            .into_iter()
            .filter(|r| origin.is_none_or(|o| r.origin == o));
        let page: Vec<ChangeRow> = matching.by_ref().take(limit).collect();
        let has_more = matching.next().is_some();
        let next = match (has_more, page.last(), scanned_to) {
            (true, Some(last), _) => last.seq,
            (_, _, Some(seq)) => seq,
            _ => cursor.unwrap_or(latest),
        };
        Ok((page, next, has_more))
    }

    /// Status without network I/O beyond a cached or first identity check.
    pub async fn status(&self, resolve: bool) -> StatusSnapshot {
        let mut out = StatusSnapshot {
            configured: self.is_configured(),
            ..StatusSnapshot::default()
        };
        if !out.configured {
            out.state = "not_configured";
            out.message = Some("no iCloud CalDAV credentials are configured".to_owned());
            return out;
        }
        let identity = if resolve {
            self.identity(false).await.ok()
        } else {
            lock(&self.inner.identity).clone()
        };
        let (state, pin) = self
            .store()
            .read(|docs| {
                Ok((
                    docs.get::<SyncState>(&SINGLETON.to_owned())?,
                    docs.get::<PrimaryPin>(&SINGLETON.to_owned())?,
                ))
            })
            .await
            .unwrap_or((None, None));
        if let Some(identity) = &identity {
            out.is_server_default = identity.is_server_default;
            out.pipeline_targets_primary = identity.pipeline_targets_primary;
            out.supports_sync = Some(identity.supports_sync);
            out.writable = Some(identity.writable);
        }
        if let Some(pin) = pin {
            out.pinned_at = Some(pin.pinned_at);
            out.repinned_at = pin.repinned_at;
        }
        if let Some(state) = &state {
            out.last_sync_at = state.last_sync_at;
            out.last_full_sync_at = state.last_full_sync_at;
            out.last_error = state.last_error.clone();
            out.event_count = state.resource_count;
            out.change_cursor = state.change_seq;
        }
        let identity_error = lock(&self.inner.identity_error).clone();
        if let Some(error) = identity_error.filter(|_| identity.is_none()) {
            out.state = "identity_error";
            out.error_code = Some(error.code());
            out.message = Some(error.to_string());
        } else if out.last_error.is_some() {
            out.state = "sync_error";
            out.message = out.last_error.clone();
        } else {
            out.state = "ready";
        }
        out
    }
}
