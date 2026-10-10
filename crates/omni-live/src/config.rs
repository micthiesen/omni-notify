//! Tracked-streamer configuration in the docstore: one `live-streamer-config`
//! row per streamer and one `live-settings` row. The UI and MCP edit it
//! through [`StreamerConfigService`], which validates every change and
//! applies it to the running live check without a restart.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use omni_api::streamer_config::{
    LiveSettings, StreamerConfigCreate, StreamerConfigPatch, StreamerConfigResponse,
    StreamerConfigView,
};
use omni_api::streamers::StreamerTier;
use omni_core::clock::SharedClock;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use omni_store::{Store, StoreError};
use omni_tasks::{AppEvent, EventBus};
use serde::{Deserialize, Serialize};

use crate::platform::{Platform, PlatformBinding};
use crate::streamers::{Roster, Streamer, js_trim, normalize_id};

const LOG: &str = "StreamerConfig";

/// Most configured streamers.
pub const MAX_STREAMERS: usize = 200;
/// Most usernames per platform per streamer.
pub const MAX_SOURCES_PER_PLATFORM: usize = 10;
/// Longest display name or username, in characters.
pub const MAX_NAME_CHARS: usize = 100;
/// Upper bound for `dggTopEmbeds`.
pub const MAX_DGG_TOP_EMBEDS: u32 = 20;

const SETTINGS_KEY: &str = "settings";

/// `live-streamer-config`: one configured streamer. `Debug` redacts the token.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerConfigRow {
    pub id: String,
    pub display_name: String,
    /// Display order (ascending).
    pub position: i64,
    #[serde(default)]
    pub youtube: Vec<String>,
    #[serde(default)]
    pub twitch: Vec<String>,
    #[serde(default)]
    pub kick: Vec<String>,
    #[serde(default)]
    pub tier: StreamerTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_notifications: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushover_token: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl std::fmt::Debug for StreamerConfigRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamerConfigRow")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("position", &self.position)
            .field("youtube", &self.youtube)
            .field("twitch", &self.twitch)
            .field("kick", &self.kick)
            .field("tier", &self.tier)
            .field("live_notifications", &self.live_notifications)
            .field(
                "pushover_token",
                &self.pushover_token.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

impl Entity for StreamerConfigRow {
    const NAME: &'static str = "live-streamer-config";
    type Key = String;

    fn key(&self) -> String {
        self.id.clone()
    }
}

/// `live-settings`: the global livestream settings (one row).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSettingsRow {
    pub key: String,
    pub dgg_top_embeds: u32,
    /// When the retired `channels.json` file was imported, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_at: Option<i64>,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for LiveSettingsRow {
    const NAME: &'static str = "live-settings";
    type Key = String;

    fn key(&self) -> String {
        self.key.clone()
    }
}

/// The live `dggTopEmbeds` value shared with the live check.
#[derive(Clone, Debug, Default)]
pub struct TopEmbeds(Arc<AtomicU32>);

impl TopEmbeds {
    pub fn new(value: u32) -> Self {
        Self(Arc::new(AtomicU32::new(value)))
    }

    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set(&self, value: u32) {
        self.0.store(value, Ordering::Relaxed);
    }
}

/// Why a configuration change or load failed.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The request is invalid; the message is safe to show.
    #[error("{0}")]
    Invalid(String),
    #[error("Unknown streamer \"{0}\"")]
    NotFound(String),
    #[error("streamer config store failed: {0}")]
    Store(#[from] StoreError),
}

impl ConfigError {
    /// A caller mistake (400/404), not a server failure.
    pub fn is_input(&self) -> bool {
        matches!(self, ConfigError::Invalid(_) | ConfigError::NotFound(_))
    }
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(message.into())
}

/// The editable fields of one streamer, after validation.
#[derive(Clone, PartialEq, Eq)]
struct Draft {
    display_name: String,
    youtube: Vec<String>,
    twitch: Vec<String>,
    kick: Vec<String>,
    tier: StreamerTier,
    live_notifications: Option<bool>,
    pushover_token: Option<String>,
}

impl std::fmt::Debug for Draft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Draft")
            .field("display_name", &self.display_name)
            .field("youtube", &self.youtube)
            .field("twitch", &self.twitch)
            .field("kick", &self.kick)
            .field("tier", &self.tier)
            .field("live_notifications", &self.live_notifications)
            .field(
                "pushover_token",
                &self.pushover_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Draft {
    fn of(row: &StreamerConfigRow) -> Self {
        Self {
            display_name: row.display_name.clone(),
            youtube: row.youtube.clone(),
            twitch: row.twitch.clone(),
            kick: row.kick.clone(),
            tier: row.tier,
            live_notifications: row.live_notifications,
            pushover_token: row.pushover_token.clone(),
        }
    }

    fn sources(&self) -> impl Iterator<Item = (Platform, &String)> {
        [
            (Platform::YouTube, &self.youtube),
            (Platform::Twitch, &self.twitch),
            (Platform::Kick, &self.kick),
        ]
        .into_iter()
        .flat_map(|(platform, names)| names.iter().map(move |name| (platform, name)))
    }
}

/// Identity of a source for duplicate checks. Twitch logins, Kick slugs and
/// YouTube `@handles` are case-insensitive; YouTube channel paths are not.
fn binding_key(platform: Platform, username: &str) -> String {
    if platform == Platform::YouTube && !username.starts_with('@') {
        format!("{platform}:{username}")
    } else {
        format!("{platform}:{}", username.to_lowercase())
    }
}

/// Usernames are interpolated into platform URLs, so each platform allows
/// only its own character set (YouTube also allows `channel/UC...` paths).
fn valid_username(platform: Platform, name: &str) -> bool {
    match platform {
        Platform::YouTube => {
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '/'))
                && !name.starts_with('/')
                && !name.ends_with('/')
                && !name
                    .split('/')
                    .any(|part| part.is_empty() || part.starts_with('.'))
        }
        Platform::Twitch => name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        Platform::Kick => name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')),
    }
}

/// Trims each username, drops case-insensitive repeats, and checks bounds.
fn clean_usernames(platform: Platform, names: Vec<String>) -> Result<Vec<String>, ConfigError> {
    let mut seen = HashSet::new();
    let mut cleaned = Vec::new();
    for name in names {
        let name = js_trim(&name).to_owned();
        if name.is_empty() {
            return Err(invalid(format!("{platform}: usernames must not be empty")));
        }
        if name.chars().count() > MAX_NAME_CHARS || !valid_username(platform, &name) {
            return Err(invalid(format!(
                "{platform}: \"{name}\" is not a valid username"
            )));
        }
        if seen.insert(binding_key(platform, &name)) {
            cleaned.push(name);
        }
    }
    if cleaned.len() > MAX_SOURCES_PER_PLATFORM {
        return Err(invalid(format!(
            "{platform}: at most {MAX_SOURCES_PER_PLATFORM} usernames"
        )));
    }
    Ok(cleaned)
}

fn clean_token(token: Option<String>) -> Result<Option<String>, ConfigError> {
    let Some(token) = token else {
        return Ok(None);
    };
    let token = js_trim(&token).to_owned();
    if token.is_empty() {
        return Ok(None);
    }
    if token.len() > 64 || !token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(invalid(
            "pushoverToken must be a Pushover application token",
        ));
    }
    Ok(Some(token))
}

/// Validates one streamer on its own.
fn validate(draft: Draft) -> Result<Draft, ConfigError> {
    let display_name = js_trim(&draft.display_name).to_owned();
    if display_name.is_empty() {
        return Err(invalid("displayName must not be blank"));
    }
    if display_name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid(format!(
            "displayName must be at most {MAX_NAME_CHARS} characters"
        )));
    }
    let cleaned = Draft {
        display_name,
        youtube: clean_usernames(Platform::YouTube, draft.youtube)?,
        twitch: clean_usernames(Platform::Twitch, draft.twitch)?,
        kick: clean_usernames(Platform::Kick, draft.kick)?,
        tier: draft.tier,
        live_notifications: draft.live_notifications,
        pushover_token: clean_token(draft.pushover_token)?,
    };
    if cleaned.sources().next().is_none() {
        return Err(invalid(
            "at least one source (youtube, twitch or kick) is required",
        ));
    }
    // The background tier always mutes live notifications; an override would
    // either contradict it or silently do nothing.
    if cleaned.tier == StreamerTier::Background && cleaned.live_notifications.is_some() {
        return Err(invalid(
            "liveNotifications cannot be set for a background streamer (the tier mutes them)",
        ));
    }
    Ok(cleaned)
}

/// Checks a validated draft against every other configured streamer.
fn check_conflicts(
    draft: &Draft,
    id: &str,
    others: &[StreamerConfigRow],
) -> Result<(), ConfigError> {
    let name = normalize_id(&draft.display_name);
    for other in others.iter().filter(|o| o.id != id) {
        if normalize_id(&other.display_name) == name || other.id == name {
            return Err(invalid(format!(
                "a streamer named \"{}\" already exists",
                other.display_name
            )));
        }
        let taken: HashSet<String> = Draft::of(other)
            .sources()
            .map(|(platform, username)| binding_key(platform, username))
            .collect();
        if let Some((platform, username)) = draft
            .sources()
            .find(|(platform, username)| taken.contains(&binding_key(*platform, username)))
        {
            return Err(invalid(format!(
                "{platform}:{username} already belongs to \"{}\"",
                other.display_name
            )));
        }
    }
    Ok(())
}

/// The API view (never the token itself).
pub fn view(row: &StreamerConfigRow) -> StreamerConfigView {
    StreamerConfigView {
        id: row.id.clone(),
        display_name: row.display_name.clone(),
        youtube: row.youtube.clone(),
        twitch: row.twitch.clone(),
        kick: row.kick.clone(),
        tier: row.tier,
        live_notifications: row.live_notifications,
        has_pushover_token: row.pushover_token.is_some(),
    }
}

/// The live-check streamer for a row; bindings ordered youtube, twitch, kick.
pub fn to_streamer(row: &StreamerConfigRow, kick_available: bool) -> Option<Streamer> {
    let bindings: Vec<PlatformBinding> = Draft::of(row)
        .sources()
        .filter(|(platform, _)| kick_available || *platform != Platform::Kick)
        .map(|(platform, username)| PlatformBinding::new(platform, username.clone()))
        .collect();
    if bindings.is_empty() {
        return None;
    }
    Some(Streamer {
        id: row.id.clone(),
        display_name: row.display_name.clone(),
        bindings,
        pushover_token: row.pushover_token.clone(),
        live_notifications: row.live_notifications,
        tier: row.tier,
        dgg: None,
        discovery_source: None,
    })
}

/// Rows for `streamers`, validated as creates, in order (tests and the
/// preview seed the store with them).
pub fn rows_for(
    streamers: Vec<StreamerConfigCreate>,
    now: i64,
) -> Result<Vec<StreamerConfigRow>, ConfigError> {
    let mut rows: Vec<StreamerConfigRow> = Vec::new();
    for (position, input) in streamers.into_iter().enumerate() {
        let draft = validate(Draft {
            display_name: input.display_name,
            youtube: input.youtube,
            twitch: input.twitch,
            kick: input.kick,
            tier: input.tier,
            live_notifications: input.live_notifications,
            pushover_token: input.pushover_token,
        })?;
        let id = normalize_id(&draft.display_name);
        check_conflicts(&draft, &id, &rows)?;
        rows.push(new_row(
            id,
            draft,
            i64::try_from(position).unwrap_or(i64::MAX),
            now,
        ));
    }
    Ok(rows)
}

fn new_row(id: String, draft: Draft, position: i64, now: i64) -> StreamerConfigRow {
    StreamerConfigRow {
        id,
        display_name: draft.display_name,
        position,
        youtube: draft.youtube,
        twitch: draft.twitch,
        kick: draft.kick,
        tier: draft.tier,
        live_notifications: draft.live_notifications,
        pushover_token: draft.pushover_token,
        created_at: now,
        updated_at: now,
        extra: Extra::new(),
    }
}

fn apply_row(row: &mut StreamerConfigRow, draft: Draft, now: i64) {
    row.display_name = draft.display_name;
    row.youtube = draft.youtube;
    row.twitch = draft.twitch;
    row.kick = draft.kick;
    row.tier = draft.tier;
    row.live_notifications = draft.live_notifications;
    row.pushover_token = draft.pushover_token;
    row.updated_at = now;
}

/// Display order: every primary streamer before every background one, then
/// by position within each tier.
fn sorted(mut rows: Vec<StreamerConfigRow>) -> Vec<StreamerConfigRow> {
    rows.sort_by(|a, b| {
        (a.tier == StreamerTier::Background)
            .cmp(&(b.tier == StreamerTier::Background))
            .then(a.position.cmp(&b.position))
            .then_with(|| a.id.cmp(&b.id))
    });
    rows
}

/// The position for `id` moving into `tier`: the boundary between the tiers,
/// so a promotion lands last among primary streamers and a demotion first
/// among background ones.
fn boundary_position(rows: &[StreamerConfigRow], id: &str, tier: StreamerTier) -> i64 {
    let others = rows.iter().filter(|r| r.id != id && r.tier == tier);
    match tier {
        StreamerTier::Primary => others.map(|r| r.position).max().map_or(0, |p| p + 1),
        StreamerTier::Background => others.map(|r| r.position).min().map_or(0, |p| p - 1),
    }
}

struct Inner {
    store: Store,
    clock: SharedClock,
    bus: Option<EventBus>,
    /// The configured streamers the live check polls (Kick filtered).
    configured: Roster,
    /// The displayed roster: configured streamers plus DGG discoveries.
    roster: Roster,
    top_embeds: TopEmbeds,
    kick_available: bool,
    /// Serializes read-validate-write cycles.
    writes: tokio::sync::Mutex<()>,
}

/// Reads and changes the tracked-streamer configuration.
#[derive(Clone)]
pub struct StreamerConfigService {
    inner: Arc<Inner>,
}

impl StreamerConfigService {
    pub fn new(
        store: Store,
        clock: SharedClock,
        bus: Option<EventBus>,
        configured: Roster,
        roster: Roster,
        top_embeds: TopEmbeds,
        kick_available: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                clock,
                bus,
                configured,
                roster,
                top_embeds,
                kick_available,
                writes: tokio::sync::Mutex::new(()),
            }),
        }
    }

    fn now(&self) -> i64 {
        self.inner.clock.now_ms()
    }

    pub fn kick_available(&self) -> bool {
        self.inner.kick_available
    }

    async fn rows(&self) -> Result<Vec<StreamerConfigRow>, ConfigError> {
        let rows = self
            .inner
            .store
            .read(|docs| docs.get_all::<StreamerConfigRow>())
            .await?;
        Ok(sorted(rows))
    }

    async fn settings_row(&self) -> Result<Option<LiveSettingsRow>, ConfigError> {
        Ok(self
            .inner
            .store
            .read(|docs| docs.get::<LiveSettingsRow>(&SETTINGS_KEY.to_owned()))
            .await?)
    }

    /// Applies the stored configuration to the live check and the roster.
    pub async fn reload(&self) -> Result<(), ConfigError> {
        let rows = self.rows().await?;
        let settings = self.settings_row().await?;
        let kick = self.inner.kick_available;
        let streamers: Vec<Streamer> = rows.iter().filter_map(|r| to_streamer(r, kick)).collect();
        if !kick && rows.iter().any(|r| !r.kick.is_empty()) {
            tracing::warn!(
                target: LOG,
                "Kick channels configured but KICK_CLIENT_ID/KICK_CLIENT_SECRET missing; skipping Kick"
            );
        }
        let configured_bindings: HashSet<String> = streamers
            .iter()
            .flat_map(|s| &s.bindings)
            .map(|b| binding_key(b.platform, &b.username))
            .collect();
        let current = self.inner.roster.snapshot();
        let mut next: Vec<Streamer> = streamers
            .iter()
            .cloned()
            .map(|mut streamer| {
                streamer.dgg = current
                    .iter()
                    .find(|c| c.id == streamer.id && c.discovery_source.is_none())
                    .and_then(|c| c.dgg);
                streamer
            })
            .collect();
        let discoveries: Vec<Streamer> = current
            .into_iter()
            .filter(|c| {
                c.discovery_source.is_some()
                    && !next.iter().any(|n| n.id == c.id)
                    && !c.bindings.iter().any(|b| {
                        configured_bindings.contains(&binding_key(b.platform, &b.username))
                    })
            })
            .collect();
        next.extend(discoveries);
        self.inner.configured.replace(streamers);
        self.inner.roster.replace(next);
        self.inner
            .top_embeds
            .set(settings.map_or(0, |s| s.dgg_top_embeds));
        if let Some(bus) = &self.inner.bus {
            bus.emit_app(AppEvent::StreamersChanged);
        }
        Ok(())
    }

    /// Every configured streamer, in order, and the settings.
    pub async fn list(&self) -> Result<StreamerConfigResponse, ConfigError> {
        let rows = self.rows().await?;
        let settings = self.settings_row().await?;
        Ok(StreamerConfigResponse {
            streamers: rows.iter().map(view).collect(),
            settings: LiveSettings {
                dgg_top_embeds: settings.map_or(0, |s| s.dgg_top_embeds),
            },
            kick_configured: self.inner.kick_available,
        })
    }

    pub async fn get(&self, id: &str) -> Result<StreamerConfigView, ConfigError> {
        let rows = self.rows().await?;
        rows.iter()
            .find(|r| r.id == id)
            .map(view)
            .ok_or_else(|| ConfigError::NotFound(id.to_owned()))
    }

    pub async fn create(
        &self,
        input: StreamerConfigCreate,
    ) -> Result<StreamerConfigView, ConfigError> {
        let _guard = self.inner.writes.lock().await;
        let rows = self.rows().await?;
        if rows.len() >= MAX_STREAMERS {
            return Err(invalid(format!("at most {MAX_STREAMERS} streamers")));
        }
        let draft = validate(Draft {
            display_name: input.display_name,
            youtube: input.youtube,
            twitch: input.twitch,
            kick: input.kick,
            tier: input.tier,
            live_notifications: input.live_notifications,
            pushover_token: input.pushover_token,
        })?;
        let id = normalize_id(&draft.display_name);
        if rows.iter().any(|r| r.id == id) {
            return Err(invalid(format!(
                "a streamer with id \"{id}\" already exists"
            )));
        }
        check_conflicts(&draft, &id, &rows)?;
        let position = rows.iter().map(|r| r.position).max().map_or(0, |p| p + 1);
        let row = new_row(id, draft, position, self.now());
        let stored = row.clone();
        self.inner
            .store
            .write(move |tx| tx.upsert(&stored, UpsertOpts::default()))
            .await?;
        tracing::info!(target: LOG, "Added streamer \"{}\"", row.display_name);
        self.reload().await?;
        Ok(view(&row))
    }

    /// Applies `patch`. A tier change to background drops a live-notification
    /// override unless the patch sets one (which is then rejected), and any
    /// tier change moves the streamer to the boundary between the tiers.
    pub async fn update(
        &self,
        id: &str,
        patch: StreamerConfigPatch,
    ) -> Result<StreamerConfigView, ConfigError> {
        let _guard = self.inner.writes.lock().await;
        let rows = self.rows().await?;
        let mut row = rows
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| ConfigError::NotFound(id.to_owned()))?;
        let mut draft = Draft::of(&row);
        if let Some(name) = patch.display_name {
            draft.display_name = name;
        }
        if let Some(names) = patch.youtube {
            draft.youtube = names;
        }
        if let Some(names) = patch.twitch {
            draft.twitch = names;
        }
        if let Some(names) = patch.kick {
            draft.kick = names;
        }
        if let Some(tier) = patch.tier {
            if tier == StreamerTier::Background && patch.live_notifications.is_none() {
                draft.live_notifications = None;
            }
            draft.tier = tier;
        }
        if let Some(live) = patch.live_notifications {
            draft.live_notifications = live;
        }
        if let Some(token) = patch.pushover_token {
            draft.pushover_token = Some(token);
        }
        let draft = validate(draft)?;
        check_conflicts(&draft, id, &rows)?;
        if draft.tier != row.tier {
            row.position = boundary_position(&rows, id, draft.tier);
        }
        apply_row(&mut row, draft, self.now());
        let stored = row.clone();
        self.inner
            .store
            .write(move |tx| tx.upsert(&stored, UpsertOpts::default()))
            .await?;
        tracing::info!(target: LOG, "Updated streamer \"{}\"", row.display_name);
        self.reload().await?;
        Ok(view(&row))
    }

    /// Removes a streamer. Its history (sessions, metrics) is kept, so
    /// re-adding the same name resumes it.
    pub async fn delete(&self, id: &str) -> Result<StreamerConfigView, ConfigError> {
        let _guard = self.inner.writes.lock().await;
        let rows = self.rows().await?;
        let row = rows
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| ConfigError::NotFound(id.to_owned()))?;
        let key = row.id.clone();
        self.inner
            .store
            .write(move |tx| tx.delete::<StreamerConfigRow>(&key))
            .await?;
        tracing::info!(target: LOG, "Removed streamer \"{}\"", row.display_name);
        self.reload().await?;
        Ok(view(&row))
    }

    /// Reorders streamers; `ids` must list every configured id exactly once.
    /// The order applies within each tier: primary streamers still list first.
    pub async fn reorder(&self, ids: Vec<String>) -> Result<(), ConfigError> {
        let _guard = self.inner.writes.lock().await;
        let rows = self.rows().await?;
        let unique: HashSet<&String> = ids.iter().collect();
        let known: HashSet<&String> = rows.iter().map(|r| &r.id).collect();
        if unique.len() != ids.len() || unique != known {
            return Err(invalid(
                "ids must list every configured streamer exactly once",
            ));
        }
        let now = self.now();
        let updated: Vec<StreamerConfigRow> = rows
            .into_iter()
            .map(|mut row| {
                let position = ids.iter().position(|id| *id == row.id).unwrap_or(0);
                row.position = i64::try_from(position).unwrap_or(i64::MAX);
                row.updated_at = now;
                row
            })
            .collect();
        self.inner
            .store
            .write(move |tx| {
                for row in &updated {
                    tx.upsert(row, UpsertOpts::default())?;
                }
                Ok::<_, StoreError>(())
            })
            .await?;
        self.reload().await
    }

    pub async fn update_settings(
        &self,
        settings: LiveSettings,
    ) -> Result<LiveSettings, ConfigError> {
        if settings.dgg_top_embeds > MAX_DGG_TOP_EMBEDS {
            return Err(invalid(format!(
                "dggTopEmbeds must be between 0 and {MAX_DGG_TOP_EMBEDS}"
            )));
        }
        let _guard = self.inner.writes.lock().await;
        let now = self.now();
        let mut row = self.settings_row().await?.unwrap_or(LiveSettingsRow {
            key: SETTINGS_KEY.to_owned(),
            dgg_top_embeds: 0,
            imported_at: None,
            updated_at: now,
            extra: Extra::new(),
        });
        row.dgg_top_embeds = settings.dgg_top_embeds;
        row.updated_at = now;
        self.inner
            .store
            .write(move |tx| tx.upsert(&row, UpsertOpts::default()))
            .await?;
        self.reload().await?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(name: &str) -> Draft {
        Draft {
            display_name: name.to_owned(),
            youtube: Vec::new(),
            twitch: vec!["a".to_owned()],
            kick: Vec::new(),
            tier: StreamerTier::Primary,
            live_notifications: None,
            pushover_token: None,
        }
    }

    #[test]
    fn validation_trims_dedupes_and_rejects_contradictions() {
        let mut d = draft("  Destiny ");
        d.youtube = vec![" @destiny".to_owned(), "@Destiny".to_owned()];
        let cleaned = validate(d).unwrap();
        assert_eq!(cleaned.display_name, "Destiny");
        assert_eq!(cleaned.youtube, ["@destiny"]);

        let mut none = draft("x");
        none.twitch.clear();
        assert!(matches!(validate(none), Err(ConfigError::Invalid(_))));

        let mut muted = draft("x");
        muted.tier = StreamerTier::Background;
        muted.live_notifications = Some(false);
        assert!(matches!(validate(muted), Err(ConfigError::Invalid(_))));

        let mut token = draft("x");
        token.pushover_token = Some("not a token!".to_owned());
        assert!(matches!(validate(token), Err(ConfigError::Invalid(_))));
        let mut blank = draft("x");
        blank.pushover_token = Some(" ".to_owned());
        assert_eq!(validate(blank).unwrap().pushover_token, None);
    }

    #[test]
    fn conflicts_cover_names_and_case_insensitive_bindings() {
        let existing = new_row("destiny".to_owned(), draft("Destiny"), 0, 0);
        let rows = vec![existing];
        assert!(check_conflicts(&draft(" destiny"), "other", &rows).is_err());
        let mut binding = draft("Other");
        binding.twitch = vec!["A".to_owned()];
        assert!(check_conflicts(&binding, "other", &rows).is_err());
        // The streamer itself never conflicts.
        assert!(check_conflicts(&draft("Destiny"), "destiny", &rows).is_ok());
    }

    #[test]
    fn kick_bindings_are_skipped_without_credentials() {
        let mut d = draft("K");
        d.twitch.clear();
        d.kick = vec!["k".to_owned()];
        let row = new_row("k".to_owned(), d, 0, 0);
        assert!(to_streamer(&row, false).is_none());
        assert_eq!(to_streamer(&row, true).unwrap().bindings.len(), 1);
    }
}
