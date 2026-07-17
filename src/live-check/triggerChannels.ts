import { Platform } from "./platforms/index.js";
import { getYouTubeLiveUrl } from "./platforms/youtube.js";
import { comparePlatformPriority, type Streamer } from "./streamers.js";

/** Channel shape consumed by the homebridge-stream-triggers Homebridge plugin. */
export type TriggerChannel = {
  key: string;
  displayName: string;
  type: "youtube" | "twitch";
  /** YouTube only: the channel live page the plugin resolves via yt-dlp. */
  url?: string;
};

/**
 * One trigger channel per streamer, on its highest-priority launchable platform
 * (YouTube preferred: it deep-links straight into the live video on tvOS).
 * Kick-only streamers are omitted — there is no tvOS app to launch.
 */
export function toTriggerChannels(streamers: Streamer[]): TriggerChannel[] {
  const channels: TriggerChannel[] = [];
  for (const streamer of streamers) {
    const binding = [...streamer.bindings]
      .sort((a, b) => comparePlatformPriority(a.platform, b.platform))
      .find((b) => b.platform === Platform.YouTube || b.platform === Platform.Twitch);
    if (!binding) continue;

    channels.push(
      binding.platform === Platform.YouTube
        ? {
            key: streamer.id,
            displayName: streamer.displayName,
            type: "youtube",
            url: getYouTubeLiveUrl(binding.username),
          }
        : { key: streamer.id, displayName: streamer.displayName, type: "twitch" },
    );
  }
  return channels;
}
