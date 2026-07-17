import { describe, expect, it } from "vitest";
import { Platform } from "./platforms/index.js";
import type { Streamer } from "./streamers.js";
import { toTriggerChannels } from "./triggerChannels.js";

function streamer(displayName: string, bindings: Streamer["bindings"]): Streamer {
  return { id: displayName.toLowerCase(), displayName, bindings };
}

describe("toTriggerChannels", () => {
  it("maps a youtube binding to a channel with the live-page url", () => {
    const channels = toTriggerChannels([
      streamer("Destiny", [{ platform: Platform.YouTube, username: "@destiny" }]),
    ]);
    expect(channels).toEqual([
      {
        key: "destiny",
        displayName: "Destiny",
        type: "youtube",
        url: "https://www.youtube.com/@destiny/live",
      },
    ]);
  });

  it("maps a twitch binding without a url", () => {
    const channels = toTriggerChannels([
      streamer("Jerma", [{ platform: Platform.Twitch, username: "jerma985" }]),
    ]);
    expect(channels).toEqual([{ key: "jerma", displayName: "Jerma", type: "twitch" }]);
  });

  it("prefers youtube over twitch for multi-platform streamers", () => {
    const channels = toTriggerChannels([
      streamer("Both", [
        { platform: Platform.Twitch, username: "both" },
        { platform: Platform.YouTube, username: "@both" },
      ]),
    ]);
    expect(channels).toHaveLength(1);
    expect(channels[0]?.type).toBe("youtube");
  });

  it("falls back to twitch when youtube is absent but kick is present", () => {
    const channels = toTriggerChannels([
      streamer("Mixed", [
        { platform: Platform.Kick, username: "mixed" },
        { platform: Platform.Twitch, username: "mixed" },
      ]),
    ]);
    expect(channels[0]?.type).toBe("twitch");
  });

  it("omits kick-only streamers", () => {
    const channels = toTriggerChannels([
      streamer("KickOnly", [{ platform: Platform.Kick, username: "kickonly" }]),
    ]);
    expect(channels).toEqual([]);
  });
});
