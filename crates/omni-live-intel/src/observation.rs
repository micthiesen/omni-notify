//! The live-check view the intelligence service consumes: a streamer and its
//! current live status, decoded from the `LiveDirectory` port's values
//! (streamer summaries and status views).

use serde::{Deserialize, Deserializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Platform {
    #[serde(rename = "youtube")]
    YouTube,
    #[serde(rename = "twitch")]
    Twitch,
    #[serde(rename = "kick")]
    Kick,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::Twitch => "twitch",
            Self::Kick => "kick",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube",
            Self::Twitch => "Twitch",
            Self::Kick => "Kick",
        }
    }

    /// `platformConfigs[platform].getLiveUrl(username)`.
    pub fn live_url(self, username: &str) -> String {
        match self {
            Self::YouTube => format!("https://www.youtube.com/{username}/live"),
            Self::Twitch => format!("https://www.twitch.tv/{username}"),
            Self::Kick => format!("https://kick.com/{username}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformBinding {
    pub platform: Platform,
    pub username: String,
    /// `urlOverride` in persisted statuses; the `LiveDirectory` views carry
    /// the resolved watch URL (`urlOverride ?? live page`) as `url`, which
    /// yields the same notification and capture URL.
    #[serde(default, alias = "url")]
    pub url_override: Option<String>,
}

impl PlatformBinding {
    pub fn new(platform: Platform, username: impl Into<String>) -> Self {
        Self {
            platform,
            username: username.into(),
            url_override: None,
        }
    }

    /// `getNotificationUrlFields(platform, username, urlOverride)`.
    pub fn notification_url(&self) -> (String, String) {
        (
            self.url_override
                .clone()
                .unwrap_or_else(|| self.platform.live_url(&self.username)),
            format!("Watch on {}", self.platform.display_name()),
        )
    }
}

/// `tier`; an absent tier is primary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum StreamerTier {
    #[default]
    #[serde(rename = "primary")]
    Primary,
    #[serde(rename = "background")]
    Background,
}

/// DGG presence for a stream embed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct DggPresence {
    #[serde(default)]
    pub hosted: bool,
    #[serde(default)]
    pub viewers: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Streamer {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub bindings: Vec<PlatformBinding>,
    #[serde(default)]
    pub tier: StreamerTier,
    #[serde(default)]
    pub dgg: Option<DggPresence>,
}

/// One per-platform observation that makes up the summed viewer count.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceObservation {
    pub platform: Platform,
    pub username: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub viewer_count: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStatus {
    pub streamer_id: String,
    pub primary: PlatformBinding,
    #[serde(default)]
    pub primary_title: String,
    /// Epoch ms; stored rows hold a Date (ISO string over JSON) or epoch ms.
    #[serde(deserialize_with = "epoch_ms")]
    pub started_at: i64,
    #[serde(default)]
    pub viewer_count: Option<f64>,
    #[serde(default)]
    pub sources: Option<Vec<SourceObservation>>,
}

/// `LiveObservation` (`wentLive` and `titleChanged` are unused by the service).
#[derive(Clone, Debug, PartialEq)]
pub struct LiveObservation {
    pub streamer: Streamer,
    pub status: LiveStatus,
}

impl LiveObservation {
    pub fn session_started_at(&self) -> i64 {
        self.status.started_at
    }

    /// The URL captured for audio: the primary's override or its live page.
    pub fn stream_url(&self) -> String {
        self.status.primary.notification_url().0
    }
}

/// The sticky primary's own count, never a sum of
/// overlapping bindings; the aggregate only for rows without sources.
pub fn viewer_count_for_anomaly(status: &LiveStatus) -> Option<f64> {
    let Some(sources) = &status.sources else {
        return status.viewer_count;
    };
    sources
        .iter()
        .find(|source| {
            source.platform == status.primary.platform && source.username == status.primary.username
        })
        .and_then(|source| source.viewer_count)
}

fn epoch_ms<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(f64),
        Text(String),
    }
    match Raw::deserialize(deserializer)? {
        #[allow(clippy::cast_possible_truncation)]
        Raw::Number(ms) if ms.is_finite() => Ok(ms.trunc() as i64),
        Raw::Number(_) => Err(serde::de::Error::custom("startedAt is not finite")),
        Raw::Text(text) => text
            .parse::<jiff::Timestamp>()
            .map(|ts| ts.as_millisecond())
            .map_err(|e| serde::de::Error::custom(format!("startedAt: {e}"))),
    }
}
