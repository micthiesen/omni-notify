//! `channels.json`: the single source of truth for tracked streamers.
//!
//! A missing file means "no streamers configured"; every other failure (bad
//! JSON, an unknown key, an empty username, a contradictory tier) is an error
//! that fails boot, because failing open would silently drop streamers or
//! un-mute ones that relied on overrides.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use omni_api::streamers::StreamerTier;
use serde_json::{Map, Value};

/// Default location relative to the working directory.
pub const DEFAULT_CONFIG_PATH: &str = "./channels.json";
/// Upper bound for `dggTopEmbeds`.
pub const MAX_DGG_TOP_EMBEDS: u32 = 20;

/// A username/handle/slug on one platform, or several.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Usernames {
    One(String),
    Many(Vec<String>),
}

impl Usernames {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Usernames::One(name) => vec![name.clone()],
            Usernames::Many(names) => names.clone(),
        }
    }
}

/// One validated `channels.json` entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelEntry {
    pub youtube: Option<Usernames>,
    pub twitch: Option<Usernames>,
    pub kick: Option<Usernames>,
    pub pushover_token: Option<String>,
    pub live_notifications: Option<bool>,
    pub tier: Option<StreamerTier>,
}

/// Display name → entry, in file order (JS object key order).
pub type ChannelsConfig = IndexMap<String, ChannelEntry>;

/// The loaded live-check configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveCheckConfig {
    pub channels: ChannelsConfig,
    /// Number of current Destiny.gg hosted/top-embed sources to track.
    pub dgg_top_embeds: u32,
}

/// Why `channels.json` could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ChannelsConfigError {
    #[error("Failed to read channels config at \"{path}\": {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Failed to parse channels config at \"{path}\": {message}")]
    Parse { path: String, message: String },
    #[error("Invalid channels config at \"{path}\": {message}")]
    Invalid { path: String, message: String },
}

/// `resolve(CHANNELS_CONFIG_PATH || "./channels.json")`.
pub fn config_path(configured: Option<&str>) -> PathBuf {
    let raw = configured
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CONFIG_PATH);
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Loads and validates `channels.json` from `path`.
pub fn load_channels_config(path: &Path) -> Result<LiveCheckConfig, ChannelsConfigError> {
    let display = path.display().to_string();
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LiveCheckConfig::default());
        }
        Err(source) => {
            return Err(ChannelsConfigError::Read {
                path: display,
                source,
            });
        }
    };
    parse_channels_config(&content).map_err(|error| match error {
        ParseFailure::Json(message) => ChannelsConfigError::Parse {
            path: display,
            message,
        },
        ParseFailure::Invalid(message) => ChannelsConfigError::Invalid {
            path: display,
            message,
        },
    })
}

/// A parse failure without the file path.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseFailure {
    Json(String),
    Invalid(String),
}

/// Validates the file contents (`loadChannelsConfig` minus the I/O).
pub fn parse_channels_config(content: &str) -> Result<LiveCheckConfig, ParseFailure> {
    let parsed: Value =
        serde_json::from_str(content).map_err(|e| ParseFailure::Json(e.to_string()))?;
    let Value::Object(object) = parsed else {
        return Err(ParseFailure::Invalid("expected a JSON object".to_owned()));
    };

    let mut issues = Vec::new();
    let mut channels = ChannelsConfig::new();
    let mut dgg_raw = None;
    for (key, value) in js_key_order(object) {
        if key == "dggTopEmbeds" {
            dgg_raw = Some(value);
            continue;
        }
        if crate::streamers::js_trim(&key).is_empty() {
            issues.push(format!("{key:?}: display name must not be blank"));
        }
        match parse_entry(&value) {
            Ok(entry) => {
                channels.insert(key, entry);
            }
            Err(entry_issues) => {
                for issue in entry_issues {
                    issues.push(format!("{key}: {issue}"));
                }
            }
        }
    }
    if !issues.is_empty() {
        return Err(ParseFailure::Invalid(issues.join("; ")));
    }

    let dgg_top_embeds = match dgg_raw {
        None | Some(Value::Null) => 0,
        Some(value) => parse_dgg_top_embeds(&value)
            .map_err(|message| ParseFailure::Invalid(format!("dggTopEmbeds {message}")))?,
    };
    Ok(LiveCheckConfig {
        channels,
        dgg_top_embeds,
    })
}

/// JS object key order: array-index keys ascending first, then insertion order.
fn js_key_order(object: Map<String, Value>) -> Vec<(String, Value)> {
    let (mut indices, rest): (Vec<_>, Vec<_>) = object
        .into_iter()
        .partition(|(key, _)| array_index(key).is_some());
    indices.sort_by_key(|(key, _)| array_index(key));
    indices.extend(rest);
    indices
}

fn array_index(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

fn parse_dgg_top_embeds(value: &Value) -> Result<u32, String> {
    let Some(number) = value.as_f64() else {
        return Err("must be a number".to_owned());
    };
    if number.fract() != 0.0 || !number.is_finite() {
        return Err("must be an integer".to_owned());
    }
    if number < 0.0 {
        return Err("must be greater than or equal to 0".to_owned());
    }
    if number > f64::from(MAX_DGG_TOP_EMBEDS) {
        return Err(format!("must not exceed {MAX_DGG_TOP_EMBEDS}"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(number as u32)
}

const ENTRY_KEYS: [&str; 6] = [
    "youtube",
    "twitch",
    "kick",
    "pushoverToken",
    "liveNotifications",
    "tier",
];

fn parse_entry(value: &Value) -> Result<ChannelEntry, Vec<String>> {
    let Value::Object(fields) = value else {
        return Err(vec!["expected an object".to_owned()]);
    };
    let mut issues = Vec::new();
    for key in fields.keys() {
        if !ENTRY_KEYS.contains(&key.as_str()) {
            issues.push(format!("unrecognized key \"{key}\""));
        }
    }
    let mut entry = ChannelEntry::default();
    let usernames = |field: &str, issues: &mut Vec<String>| match fields.get(field) {
        None => None,
        Some(value) => match parse_usernames(value) {
            Ok(names) => Some(names),
            Err(message) => {
                issues.push(format!("{field}: {message}"));
                None
            }
        },
    };
    entry.youtube = usernames("youtube", &mut issues);
    entry.twitch = usernames("twitch", &mut issues);
    entry.kick = usernames("kick", &mut issues);
    match fields.get("pushoverToken") {
        None => {}
        Some(Value::String(token)) => entry.pushover_token = Some(token.clone()),
        Some(_) => issues.push("pushoverToken: expected a string".to_owned()),
    }
    match fields.get("liveNotifications") {
        None => {}
        Some(Value::Bool(flag)) => entry.live_notifications = Some(*flag),
        Some(_) => issues.push("liveNotifications: expected a boolean".to_owned()),
    }
    match fields.get("tier") {
        None => {}
        Some(Value::String(tier)) if tier == "primary" => entry.tier = Some(StreamerTier::Primary),
        Some(Value::String(tier)) if tier == "background" => {
            entry.tier = Some(StreamerTier::Background);
        }
        Some(_) => issues.push("tier: expected \"primary\" or \"background\"".to_owned()),
    }

    let has_platform = ["youtube", "twitch", "kick"]
        .iter()
        .any(|field| fields.contains_key(*field));
    if !has_platform {
        issues.push("must specify at least one platform (youtube, twitch, or kick)".to_owned());
    }
    // The background tier already implies liveNotifications: false, so an
    // explicit value is either contradictory or redundant: both fail loudly.
    if entry.tier == Some(StreamerTier::Background) {
        match entry.live_notifications {
            Some(true) => issues.push(
                "liveNotifications: liveNotifications: true contradicts the background tier, which mutes live notifications.".to_owned(),
            ),
            Some(false) => issues.push(
                "liveNotifications: liveNotifications: false is redundant, the background tier already mutes live notifications.".to_owned(),
            ),
            None => {}
        }
    }
    if issues.is_empty() {
        Ok(entry)
    } else {
        Err(issues)
    }
}

fn parse_usernames(value: &Value) -> Result<Usernames, String> {
    match value {
        Value::String(name) if name.is_empty() => Err("must not be empty".to_owned()),
        Value::String(name) => Ok(Usernames::One(name.clone())),
        Value::Array(items) if items.is_empty() => Err("must not be empty".to_owned()),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(name) if name.is_empty() => Err("must not be empty".to_owned()),
                Value::String(name) => Ok(name.clone()),
                _ => Err("expected a string or an array of strings".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Usernames::Many),
        _ => Err("expected a string or an array of strings".to_owned()),
    }
}
