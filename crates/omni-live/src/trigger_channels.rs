//! Channels for the homebridge-stream-triggers plugin.

use omni_api::streamers::{TriggerChannel, TriggerChannelType};

use crate::platform::Platform;
use crate::streamers::Streamer;

/// One channel per streamer on its highest-priority platform (YouTube
/// deep-links straight into the live video on tvOS).
pub fn to_trigger_channels(streamers: &[Streamer]) -> Vec<TriggerChannel> {
    streamers
        .iter()
        .filter_map(|streamer| {
            let binding = streamer
                .bindings
                .iter()
                .min_by_key(|binding| binding.platform.priority())?;
            let (kind, url) = match binding.platform {
                Platform::YouTube => (TriggerChannelType::Youtube, Some(binding.watch_url())),
                Platform::Twitch => (TriggerChannelType::Twitch, None),
                Platform::Kick => (TriggerChannelType::Kick, Some(binding.watch_url())),
            };
            Some(TriggerChannel {
                key: streamer.id.clone(),
                display_name: streamer.display_name.clone(),
                kind,
                url,
            })
        })
        .collect()
}
