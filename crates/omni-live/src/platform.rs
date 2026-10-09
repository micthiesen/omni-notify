//! Platforms, bindings and fetched statuses.

use serde::{Deserialize, Serialize};

/// A supported streaming platform; serialized lowercase as persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    #[serde(rename = "youtube")]
    YouTube,
    Twitch,
    Kick,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::YouTube, Platform::Twitch, Platform::Kick];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::YouTube => "youtube",
            Platform::Twitch => "twitch",
            Platform::Kick => "kick",
        }
    }

    /// Case-sensitive parse of the persisted name.
    pub fn parse(value: &str) -> Option<Platform> {
        match value {
            "youtube" => Some(Platform::YouTube),
            "twitch" => Some(Platform::Twitch),
            "kick" => Some(Platform::Kick),
            _ => None,
        }
    }

    /// `"YouTube" | "Twitch" | "Kick"` for notification link titles.
    pub fn display_name(self) -> &'static str {
        match self {
            Platform::YouTube => "YouTube",
            Platform::Twitch => "Twitch",
            Platform::Kick => "Kick",
        }
    }

    /// The channel's live page.
    pub fn live_url(self, username: &str) -> String {
        match self {
            Platform::YouTube => format!("https://www.youtube.com/{username}/live"),
            Platform::Twitch => format!("https://www.twitch.tv/{username}"),
            Platform::Kick => format!("https://kick.com/{username}"),
        }
    }

    /// Tiebreak order when several bindings go live in the same tick
    /// (YouTube, Twitch, Kick).
    pub fn priority(self) -> u8 {
        match self {
            Platform::YouTube => 0,
            Platform::Twitch => 1,
            Platform::Kick => 2,
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One platform account of a streamer.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformBinding {
    pub platform: Platform,
    pub username: String,
    /// Exact media URL for transient discovery sources such as YouTube videos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

    /// `urlOverride ?? getLiveUrl(username)`.
    pub fn watch_url(&self) -> String {
        self.url_override
            .clone()
            .unwrap_or_else(|| self.platform.live_url(&self.username))
    }

    /// Same platform and username (the override is presentation only).
    pub fn same_account(&self, other: &PlatformBinding) -> bool {
        self.platform == other.platform && self.username == other.username
    }

    /// `url` and `url_title` notification fields.
    pub fn notification_url_fields(&self) -> NotificationUrlFields {
        NotificationUrlFields {
            url: self.watch_url(),
            url_title: format!("Watch on {}", self.platform.display_name()),
        }
    }
}

/// Pushover `url` / `url_title`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationUrlFields {
    pub url: String,
    pub url_title: String,
}

/// A live observation of one binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedLive {
    pub title: String,
    pub viewer_count: Option<i64>,
    pub category: Option<String>,
    /// Source-reported stream start (discovery feeds).
    pub started_at: Option<String>,
}

/// The outcome of checking one binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchedStatus {
    Live(FetchedLive),
    Offline,
    Unknown { error: String },
}

impl FetchedStatus {
    pub fn live(title: impl Into<String>, viewer_count: Option<i64>) -> Self {
        FetchedStatus::Live(FetchedLive {
            title: title.into(),
            viewer_count,
            category: None,
            started_at: None,
        })
    }

    pub fn unknown(error: impl Into<String>) -> Self {
        FetchedStatus::Unknown {
            error: error.into(),
        }
    }
}

/// A JS number (possibly fractional) reported by a platform API, as the
/// integer count this service stores. Platforms report whole counts; a
/// fractional value is rounded rather than rejected.
pub(crate) fn js_count(value: f64) -> Option<i64> {
    if !value.is_finite() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(value.round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_urls_match_the_platform_configs() {
        assert_eq!(
            Platform::YouTube.live_url("@destiny"),
            "https://www.youtube.com/@destiny/live"
        );
        assert_eq!(
            Platform::Twitch.live_url("jerma985"),
            "https://www.twitch.tv/jerma985"
        );
        assert_eq!(
            Platform::Kick.live_url("destiny"),
            "https://kick.com/destiny"
        );
    }

    #[test]
    fn notification_fields_prefer_the_override() {
        let mut binding = PlatformBinding::new(Platform::YouTube, "abc");
        binding.url_override = Some("https://www.youtube.com/watch?v=abc".into());
        assert_eq!(
            binding.notification_url_fields(),
            NotificationUrlFields {
                url: "https://www.youtube.com/watch?v=abc".into(),
                url_title: "Watch on YouTube".into(),
            }
        );
    }

    #[test]
    fn platforms_serialize_lowercase() {
        assert_eq!(
            serde_json::to_string(&Platform::YouTube).unwrap(),
            "\"youtube\""
        );
        assert_eq!(
            serde_json::from_str::<Platform>("\"kick\"").unwrap(),
            Platform::Kick
        );
    }
}
