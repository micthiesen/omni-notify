import { Platform } from "./platforms/index.js";
import { getKickLiveUrl } from "./platforms/kick.js";
import { getYouTubeLiveUrl } from "./platforms/youtube.js";
import { comparePlatformPriority, type Streamer } from "./streamers.js";

/** Channel shape consumed by the homebridge-stream-triggers Homebridge plugin. */
export type TriggerChannel = {
  key: string;
  displayName: string;
  type: "youtube" | "twitch" | "kick";
  /**
   * youtube: the channel live page the plugin resolves via yt-dlp.
   * kick: the channel's universal link (https://kick.com/<user>) — the plugin
   * attempts it as a tvOS deep link and falls back to opening the Kick app.
   */
  url?: string;
};

/**
 * One trigger channel per streamer, on its highest-priority platform
 * (YouTube preferred: it deep-links straight into the live video on tvOS).
 */
export function toTriggerChannels(streamers: Streamer[]): TriggerChannel[] {
  const channels: TriggerChannel[] = [];
  for (const streamer of streamers) {
    const binding = [...streamer.bindings].sort((a, b) =>
      comparePlatformPriority(a.platform, b.platform),
    )[0];
    if (!binding) continue;

    const base = { key: streamer.id, displayName: streamer.displayName };
    switch (binding.platform) {
      case Platform.YouTube:
        channels.push({
          ...base,
          type: "youtube",
          url: getYouTubeLiveUrl(binding.username),
        });
        break;
      case Platform.Twitch:
        channels.push({ ...base, type: "twitch" });
        break;
      case Platform.Kick:
        channels.push({ ...base, type: "kick", url: getKickLiveUrl(binding.username) });
        break;
    }
  }
  return channels;
}
