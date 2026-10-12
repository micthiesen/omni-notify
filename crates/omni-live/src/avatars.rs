//! Streamer avatars (entity `streamer-avatar`): each platform account's
//! profile picture URL, refreshed by the `StreamerAvatars` task and read by
//! the streamer views, notifications and iOS widgets.
//!
//! The task fetches at most [`MAX_FETCHES_PER_RUN`] accounts per run:
//! configured accounts first (missing, or older than [`REFRESH_AFTER_MS`]),
//! then accounts of current DGG discoveries that have none. A failed lookup
//! keeps the last known URL and is retried no sooner than [`RETRY_AFTER_MS`].
//! Lookups are reads, so `SideEffectMode` does not apply.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::StreamExt;
use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_store::Store;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};
use serde::{Deserialize, Serialize};

use crate::error::LiveError;
use crate::identity::binding_key;
use crate::platform::{Platform, PlatformBinding};
use crate::platforms::Platforms;
use crate::status::StreamerStatus;
use crate::streamers::{Roster, Streamer};

const LOG: &str = "StreamerAvatars";
/// Task name (persisted in run history).
pub const TASK_NAME: &str = "StreamerAvatars";
/// Every 10 minutes.
pub const SCHEDULE: &str = "0 */10 * * * *";
const JITTER: std::time::Duration = std::time::Duration::from_secs(30);
/// Lookups per run.
pub const MAX_FETCHES_PER_RUN: usize = 10;
const FETCH_CONCURRENCY: usize = 3;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A known avatar is refreshed after a week.
pub const REFRESH_AFTER_MS: i64 = 7 * DAY_MS;
/// A failed lookup, or an account without an avatar, waits a day.
pub const RETRY_AFTER_MS: i64 = DAY_MS;
/// Rows for accounts no longer tracked are dropped after 30 idle days. Without
/// Kick credentials, Kick bindings leave the roster and are never looked up,
/// so their rows are pruned this way too.
pub const PRUNE_AFTER_MS: i64 = 30 * DAY_MS;
const MAX_URL_LEN: usize = 2048;

/// One platform account's avatar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerAvatar {
    /// Canonical account key ([`binding_key`]), e.g. `twitch:vinesauce`.
    pub binding: String,
    pub platform: Platform,
    pub username: String,
    /// The last known https avatar URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The last lookup that answered (found, or confirmed absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    /// The last failed lookup after `fetchedAt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<i64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for StreamerAvatar {
    const NAME: &'static str = "streamer-avatar";
    type Key = String;

    fn key(&self) -> String {
        self.binding.clone()
    }
}

/// A failed avatar lookup.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct AvatarError {
    pub message: String,
}

/// Looks up account avatars (the platform clients in production).
pub trait AvatarFetcher: Send + Sync {
    /// Whether lookups for `platform` are possible (Kick needs credentials).
    fn supports(&self, platform: Platform) -> bool;
    /// The avatar URL, or `None` when the account has none or does not exist.
    fn fetch<'a>(
        &'a self,
        binding: &'a PlatformBinding,
    ) -> BoxFuture<'a, Result<Option<String>, AvatarError>>;
}

impl AvatarFetcher for Platforms {
    fn supports(&self, platform: Platform) -> bool {
        platform != Platform::Kick || self.kick().has_credentials()
    }

    fn fetch<'a>(
        &'a self,
        binding: &'a PlatformBinding,
    ) -> BoxFuture<'a, Result<Option<String>, AvatarError>> {
        Box::pin(async move {
            let username = binding.username.as_str();
            match binding.platform {
                Platform::Twitch => self
                    .twitch()
                    .fetch_avatar(username)
                    .await
                    .map_err(|e| AvatarError { message: e.message }),
                Platform::Kick => self
                    .kick()
                    .fetch_avatar(username)
                    .await
                    .map_err(|message| AvatarError { message }),
                Platform::YouTube => self
                    .youtube()
                    .fetch_avatar(username)
                    .await
                    .map_err(|e| AvatarError { message: e.message }),
            }
        })
    }
}

/// Kick's stand-in picture for accounts without one; initials read better.
fn is_placeholder(url: &str) -> bool {
    url.contains("/img/default-profile-pic")
}

/// An absolute https URL of bounded length that is not a platform
/// placeholder, else `None`.
pub fn checked_avatar_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_LEN || is_placeholder(trimmed) {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    (url.scheme() == "https" && url.host_str().is_some()).then(|| url.to_string())
}

/// Pure: whether an account's avatar should be looked up at `now`.
pub fn is_due(row: Option<&StreamerAvatar>, now: i64) -> bool {
    let Some(row) = row else {
        return true;
    };
    if row
        .failed_at
        .is_some_and(|failed| now - failed < RETRY_AFTER_MS)
    {
        return false;
    }
    match row.fetched_at {
        None => true,
        Some(at) if row.url.is_none() => now - at >= RETRY_AFTER_MS,
        Some(at) => now - at >= REFRESH_AFTER_MS,
    }
}

/// Pure: the accounts to look up this run, configured streamers first, then
/// current discoveries (YouTube discoveries are videos, not channels).
pub fn select_due(
    configured: &[Streamer],
    roster: &[Streamer],
    rows: &HashMap<String, StreamerAvatar>,
    supports: impl Fn(Platform) -> bool,
    now: i64,
    limit: usize,
) -> Vec<PlatformBinding> {
    let configured_bindings = configured.iter().flat_map(|s| s.bindings.iter());
    let discovered_bindings = roster
        .iter()
        .filter(|s| s.discovery_source.is_some())
        .flat_map(|s| s.bindings.iter())
        .filter(|b| b.platform != Platform::YouTube);
    let mut seen = HashSet::new();
    configured_bindings
        .chain(discovered_bindings)
        .filter(|b| supports(b.platform))
        .filter(|b| {
            let key = binding_key(b);
            seen.insert(key.clone()) && is_due(rows.get(&key), now)
        })
        .take(limit)
        .map(|b| PlatformBinding::new(b.platform, b.username.clone()))
        .collect()
}

/// Pure: a streamer's avatar from `urls` (by account key), trying
/// `preferred` (the live primary) first, then its bindings in order.
pub fn resolve_avatar(
    streamer: &Streamer,
    preferred: Option<&PlatformBinding>,
    urls: &HashMap<String, String>,
) -> Option<String> {
    preferred
        .into_iter()
        .chain(streamer.bindings.iter())
        .find_map(|binding| urls.get(&binding_key(binding)).cloned())
}

fn url_map(rows: Vec<StreamerAvatar>) -> HashMap<String, String> {
    rows.into_iter()
        .filter_map(|row| row.url.map(|url| (row.binding, url)))
        .filter(|(_, url)| !is_placeholder(url))
        .collect()
}

/// Every known avatar URL by account key (one scan).
pub(crate) fn load_urls(
    docs: &omni_store::Docs<'_>,
) -> Result<HashMap<String, String>, omni_store::StoreError> {
    Ok(url_map(docs.get_all::<StreamerAvatar>()?))
}

/// Each streamer's avatar URL by streamer id, preferring a live streamer's
/// primary account. Streamers without one are absent. One store read.
pub async fn avatar_urls(
    store: &Store,
    streamers: &[Streamer],
) -> Result<HashMap<String, String>, LiveError> {
    let streamers = streamers.to_vec();
    store
        .read(move |docs| {
            let urls = load_urls(docs)?;
            let mut resolved = HashMap::new();
            for streamer in &streamers {
                let primary = match docs.get::<StreamerStatus>(&streamer.id)? {
                    Some(StreamerStatus::Live(live)) => Some(live.primary),
                    _ => None,
                };
                if let Some(url) = resolve_avatar(streamer, primary.as_ref(), &urls) {
                    resolved.insert(streamer.id.clone(), url);
                }
            }
            Ok(resolved)
        })
        .await
        .map_err(LiveError::persistence("read streamer avatars"))
}

/// `primary` first, then the streamer's bindings (avatar lookup order).
pub fn avatar_candidates(streamer: &Streamer, primary: &PlatformBinding) -> Vec<PlatformBinding> {
    std::iter::once(primary)
        .chain(
            streamer
                .bindings
                .iter()
                .filter(|b| !b.same_account(primary)),
        )
        .cloned()
        .collect()
}

/// The avatar of the first of `bindings` that has one (notifications). A
/// failed read is logged and yields `None`: an avatar never blocks an alert.
pub async fn avatar_for(store: &Store, bindings: &[PlatformBinding]) -> Option<String> {
    let keys: Vec<String> = bindings.iter().map(binding_key).collect();
    let read = store
        .read(move |docs| {
            for key in &keys {
                if let Some(url) = docs.get::<StreamerAvatar>(key)?.and_then(|row| row.url) {
                    return Ok(Some(url));
                }
            }
            Ok(None)
        })
        .await;
    match read {
        Ok(url) => url,
        Err(error) => {
            tracing::debug!(target: LOG, "Avatar lookup failed: {error}");
            None
        }
    }
}

/// What one refresh did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefreshSummary {
    pub attempted: usize,
    pub found: usize,
    pub failed: usize,
    pub pruned: usize,
}

/// One refresh pass over the roster (the task and tests).
pub struct AvatarRefresh {
    store: Store,
    clock: SharedClock,
    roster: Roster,
    configured: Roster,
    fetcher: Arc<dyn AvatarFetcher>,
}

impl AvatarRefresh {
    pub fn new(
        store: Store,
        clock: SharedClock,
        roster: Roster,
        configured: Roster,
        fetcher: Arc<dyn AvatarFetcher>,
    ) -> Self {
        Self {
            store,
            clock,
            roster,
            configured,
            fetcher,
        }
    }

    pub async fn run(&self) -> Result<RefreshSummary, LiveError> {
        let now = self.clock.now_ms();
        let rows: HashMap<String, StreamerAvatar> = self
            .store
            .read(|docs| docs.get_all::<StreamerAvatar>())
            .await
            .map_err(LiveError::persistence("list streamer avatars"))?
            .into_iter()
            .map(|row| (row.binding.clone(), row))
            .collect();
        let configured = self.configured.snapshot();
        let roster = self.roster.snapshot();
        let due = select_due(
            &configured,
            &roster,
            &rows,
            |platform| self.fetcher.supports(platform),
            now,
            MAX_FETCHES_PER_RUN,
        );

        let lookups: Vec<_> = due
            .iter()
            .map(|binding| async move { (binding, self.fetcher.fetch(binding).await) })
            .collect();
        let results: Vec<_> = futures::stream::iter(lookups)
            .buffer_unordered(FETCH_CONCURRENCY)
            .collect()
            .await;

        let mut summary = RefreshSummary {
            attempted: results.len(),
            ..RefreshSummary::default()
        };
        let mut first_failure = None;
        let mut updates = Vec::with_capacity(results.len());
        for (binding, result) in results {
            let key = binding_key(binding);
            let mut row = rows.get(&key).cloned().unwrap_or_else(|| StreamerAvatar {
                binding: key.clone(),
                platform: binding.platform,
                username: binding.username.clone(),
                url: None,
                fetched_at: None,
                failed_at: None,
                extra: Extra::new(),
            });
            match result {
                Ok(url) => {
                    row.url = url.as_deref().and_then(checked_avatar_url);
                    row.fetched_at = Some(now);
                    row.failed_at = None;
                    if row.url.is_some() {
                        summary.found += 1;
                    }
                }
                Err(error) => {
                    // Keep the last known URL; retry after RETRY_AFTER_MS.
                    row.failed_at = Some(now);
                    summary.failed += 1;
                    first_failure.get_or_insert(format!("{key}: {error}"));
                }
            }
            updates.push(row);
        }

        let tracked: HashSet<String> = configured
            .iter()
            .chain(roster.iter())
            .flat_map(|s| s.bindings.iter().map(binding_key))
            .collect();
        let prune: Vec<String> = rows
            .values()
            .filter(|row| !tracked.contains(&row.binding))
            .filter(|row| {
                let last = row.fetched_at.max(row.failed_at).unwrap_or(0);
                now - last >= PRUNE_AFTER_MS
            })
            .map(|row| row.binding.clone())
            .collect();
        summary.pruned = prune.len();

        if !updates.is_empty() || !prune.is_empty() {
            self.store
                .write(move |tx| {
                    for row in &updates {
                        tx.upsert(row, UpsertOpts::default())?;
                    }
                    for key in &prune {
                        tx.delete::<StreamerAvatar>(key)?;
                    }
                    Ok(())
                })
                .await
                .map_err(LiveError::persistence("upsert streamer avatars"))?;
        }

        if let Some(first) = first_failure {
            // WARN, not ERROR: a missing avatar is cosmetic and must not alert.
            tracing::warn!(
                target: LOG,
                "{} of {} avatar lookups failed (retrying after a day); first: {first}",
                summary.failed,
                summary.attempted
            );
        }
        if summary.attempted > 0 || summary.pruned > 0 {
            tracing::info!(
                target: LOG,
                "Looked up {} avatars: {} found, {} failed, {} pruned",
                summary.attempted,
                summary.found,
                summary.failed,
                summary.pruned
            );
        }
        Ok(summary)
    }
}

/// The scheduled task over an [`AvatarRefresh`].
pub struct StreamerAvatarsTask {
    refresh: Arc<AvatarRefresh>,
    schedule: CronSchedule,
}

impl StreamerAvatarsTask {
    pub fn new(
        refresh: Arc<AvatarRefresh>,
        tz: &TimeZone,
    ) -> Result<Self, omni_tasks::InvalidScheduleError> {
        Ok(Self {
            refresh,
            schedule: CronSchedule::parse(SCHEDULE, tz)?,
        })
    }
}

impl Task for StreamerAvatarsTask {
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
        Box::pin(async move {
            self.refresh
                .run()
                .await
                .map(|_| ())
                .map_err(TaskError::from_error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(url: Option<&str>, fetched_at: Option<i64>, failed_at: Option<i64>) -> StreamerAvatar {
        StreamerAvatar {
            binding: "twitch:a".into(),
            platform: Platform::Twitch,
            username: "a".into(),
            url: url.map(str::to_owned),
            fetched_at,
            failed_at,
            extra: Extra::new(),
        }
    }

    const NOW: i64 = 100 * DAY_MS;

    #[test]
    fn a_missing_row_is_due() {
        assert!(is_due(None, NOW));
    }

    #[test]
    fn a_known_avatar_refreshes_after_a_week() {
        let url = Some("https://x/a.png");
        assert!(!is_due(Some(&row(url, Some(NOW - 6 * DAY_MS), None)), NOW));
        assert!(is_due(Some(&row(url, Some(NOW - 7 * DAY_MS), None)), NOW));
    }

    #[test]
    fn an_account_without_an_avatar_is_rechecked_daily() {
        assert!(!is_due(Some(&row(None, Some(NOW - DAY_MS + 1), None)), NOW));
        assert!(is_due(Some(&row(None, Some(NOW - DAY_MS), None)), NOW));
    }

    #[test]
    fn a_failure_waits_a_day_even_when_the_avatar_is_stale() {
        let stale = Some(NOW - 30 * DAY_MS);
        let url = Some("https://x/a.png");
        assert!(!is_due(Some(&row(url, stale, Some(NOW - 1000))), NOW));
        assert!(is_due(Some(&row(url, stale, Some(NOW - DAY_MS))), NOW));
        assert!(!is_due(Some(&row(None, None, Some(NOW - 1000))), NOW));
    }

    #[test]
    fn only_absolute_https_urls_are_kept() {
        assert_eq!(
            checked_avatar_url(" https://cdn.example/a.png "),
            Some("https://cdn.example/a.png".into())
        );
        assert_eq!(checked_avatar_url("http://cdn.example/a.png"), None);
        assert_eq!(checked_avatar_url("javascript:alert(1)"), None);
        assert_eq!(checked_avatar_url("/a.png"), None);
        assert_eq!(checked_avatar_url(""), None);
        assert_eq!(
            checked_avatar_url("https://kick.com/img/default-profile-pictures/default2.jpeg"),
            None
        );
        let long = format!("https://cdn.example/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(checked_avatar_url(&long), None);
    }

    fn streamer(id: &str, bindings: &[(Platform, &str)]) -> Streamer {
        Streamer::new(
            id,
            id,
            bindings
                .iter()
                .map(|(p, u)| PlatformBinding::new(*p, *u))
                .collect(),
            omni_api::streamers::StreamerTier::Primary,
        )
    }

    #[test]
    fn resolves_the_preferred_binding_first_then_binding_order() {
        let s = streamer(
            "s",
            &[
                (Platform::YouTube, "@S"),
                (Platform::Twitch, "s"),
                (Platform::Kick, "s"),
            ],
        );
        let mut urls = HashMap::new();
        urls.insert("twitch:s".to_owned(), "https://t/s".to_owned());
        urls.insert("kick:s".to_owned(), "https://k/s".to_owned());
        assert_eq!(resolve_avatar(&s, None, &urls), Some("https://t/s".into()));
        let kick = PlatformBinding::new(Platform::Kick, "S");
        assert_eq!(
            resolve_avatar(&s, Some(&kick), &urls),
            Some("https://k/s".into())
        );
        urls.insert("youtube:@s".to_owned(), "https://y/s".to_owned());
        assert_eq!(resolve_avatar(&s, None, &urls), Some("https://y/s".into()));
        assert_eq!(resolve_avatar(&streamer("x", &[]), None, &urls), None);
    }

    #[test]
    fn selects_configured_accounts_before_discoveries_and_skips_youtube_videos() {
        let configured = vec![streamer(
            "c",
            &[(Platform::Twitch, "c"), (Platform::Kick, "c")],
        )];
        let mut video = streamer("dgg:youtube:v", &[(Platform::YouTube, "v1")]);
        video.discovery_source = Some(crate::streamers::DiscoverySource::Dgg);
        let mut kick = streamer("dgg:kick:d", &[(Platform::Kick, "d")]);
        kick.discovery_source = Some(crate::streamers::DiscoverySource::Dgg);
        let mut roster = configured.clone();
        roster.push(video);
        roster.push(kick);
        let keys = |due: Vec<PlatformBinding>| due.iter().map(binding_key).collect::<Vec<_>>();

        let all = select_due(&configured, &roster, &HashMap::new(), |_| true, NOW, 10);
        assert_eq!(keys(all), ["twitch:c", "kick:c", "kick:d"]);
        let no_kick = select_due(
            &configured,
            &roster,
            &HashMap::new(),
            |p| p != Platform::Kick,
            NOW,
            10,
        );
        assert_eq!(keys(no_kick), ["twitch:c"]);
        let limited = select_due(&configured, &roster, &HashMap::new(), |_| true, NOW, 1);
        assert_eq!(keys(limited), ["twitch:c"]);
        let mut fresh = HashMap::new();
        fresh.insert(
            "twitch:c".to_owned(),
            row(Some("https://t/c"), Some(NOW), None),
        );
        let due = select_due(&configured, &roster, &fresh, |_| true, NOW, 10);
        assert_eq!(keys(due), ["kick:c", "kick:d"]);
    }
}
