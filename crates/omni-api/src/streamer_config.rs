//! Tracked-streamer configuration (stored in the database).
//!
//! `GET /api/streamer-config`, `POST /api/streamer-config/streamers`,
//! `PATCH|DELETE /api/streamer-config/streamers/:id`,
//! `PUT /api/streamer-config/order` and `PUT /api/streamer-config/settings`.
//!
//! Pushover application tokens are write-only: views report only whether one
//! is set.

use serde::{Deserialize, Deserializer, Serialize};

use crate::streamers::StreamerTier;

/// One configured streamer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerConfigView {
    /// Stable id (the normalized display name at creation).
    pub id: String,
    pub display_name: String,
    pub youtube: Vec<String>,
    pub twitch: Vec<String>,
    pub kick: Vec<String>,
    pub tier: StreamerTier,
    /// `None` uses the tier default (on for primary, off for background).
    pub live_notifications: Option<bool>,
    pub has_pushover_token: bool,
}

/// `GET /api/streamer-config`: streamers in display order and global settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerConfigResponse {
    pub streamers: Vec<StreamerConfigView>,
    pub settings: LiveSettings,
    /// Kick bindings are only polled when Kick credentials are configured.
    pub kick_configured: bool,
}

/// Global livestream settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSettings {
    /// Current Destiny.gg hosted/top embeds tracked as transient streamers (0-20).
    pub dgg_top_embeds: u32,
}

/// `POST /api/streamer-config/streamers`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerConfigCreate {
    pub display_name: String,
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
    /// An empty string means no token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushover_token: Option<String>,
}

/// `PATCH /api/streamer-config/streamers/:id`: absent fields are unchanged.
/// A platform list replaces that platform's usernames.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerConfigPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twitch: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kick: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<StreamerTier>,
    /// Absent: unchanged; `null`: back to the tier default; a boolean: override.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub live_notifications: Option<Option<bool>>,
    /// Absent: unchanged; `""`: removed; anything else: replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushover_token: Option<String>,
}

/// Distinguishes an explicit `null` from an absent field.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// `PUT /api/streamer-config/order`: every configured id, in the new order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerConfigOrder {
    pub ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_distinguishes_null_from_absent() {
        let absent: StreamerConfigPatch = serde_json::from_str("{}").unwrap_or_default();
        assert_eq!(absent.live_notifications, None);
        let null: StreamerConfigPatch =
            serde_json::from_str(r#"{"liveNotifications":null}"#).unwrap_or_default();
        assert_eq!(null.live_notifications, Some(None));
        let off: StreamerConfigPatch =
            serde_json::from_str(r#"{"liveNotifications":false}"#).unwrap_or_default();
        assert_eq!(off.live_notifications, Some(Some(false)));
    }
}
