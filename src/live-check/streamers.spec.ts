import { describe, expect, it } from "vitest";
import type { ChannelsConfig } from "./channelsConfig.js";
import { Platform } from "./platforms/index.js";
import type { Streamer } from "./streamers.js";
import {
  BACKGROUND_POLL_FACTOR,
  buildStreamers,
  dropPlatformBindings,
  isStreamerDue,
  normalizeId,
} from "./streamers.js";

describe("normalizeId", () => {
  it("lowercases and trims display names", () => {
    expect(normalizeId("  Destiny  ")).toBe("destiny");
    expect(normalizeId("DESTINY")).toBe("destiny");
  });
});

describe("buildStreamers", () => {
  it("builds bindings from an entry's platform fields", () => {
    const config: ChannelsConfig = {
      Destiny: { youtube: "@destiny2", kick: "destiny" },
    };
    const result = buildStreamers(config);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("destiny");
    expect(result[0].displayName).toBe("Destiny");
    expect(result[0].bindings).toEqual([
      { platform: Platform.YouTube, username: "@destiny2" },
      { platform: Platform.Kick, username: "destiny" },
    ]);
  });

  it("always orders bindings youtube → twitch → kick regardless of the entry's field order", () => {
    const config: ChannelsConfig = {
      Destiny: { kick: "destiny", youtube: "@destiny2", twitch: "destinytv" },
    };
    const result = buildStreamers(config);
    expect(result[0].bindings.map((b) => b.platform)).toEqual([
      Platform.YouTube,
      Platform.Twitch,
      Platform.Kick,
    ]);
  });

  it("accepts a string[] for multiple usernames on one platform", () => {
    const config: ChannelsConfig = {
      Destiny: { twitch: ["destiny", "destinyalt"] },
    };
    const result = buildStreamers(config);
    expect(result[0].bindings).toEqual([
      { platform: Platform.Twitch, username: "destiny" },
      { platform: Platform.Twitch, username: "destinyalt" },
    ]);
  });

  it("keeps distinct streamers for distinct entries", () => {
    const config: ChannelsConfig = {
      Shroud: { twitch: "shroud" },
      Destiny: { kick: "destiny" },
    };
    const result = buildStreamers(config);
    expect(result.map((s) => s.displayName).sort()).toEqual(["Destiny", "Shroud"]);
  });

  it("throws on a platform binding duplicated across entries", () => {
    const config: ChannelsConfig = {
      Shroud: { twitch: "shroud" },
      OtherName: { twitch: "shroud" },
    };
    expect(() => buildStreamers(config)).toThrow(/Duplicate platform binding/);
  });

  it("throws on a platform binding duplicated within one entry's array", () => {
    const config: ChannelsConfig = {
      Shroud: { twitch: ["shroud", "shroud"] },
    };
    expect(() => buildStreamers(config)).toThrow(/Duplicate platform binding/);
  });

  it("throws on a duplicate across a string entry and another entry's array", () => {
    const config: ChannelsConfig = {
      Shroud: { twitch: "shroud" },
      OtherName: { twitch: ["shroud", "other"] },
    };
    expect(() => buildStreamers(config)).toThrow(/Duplicate platform binding/);
  });

  it("throws when two entries normalize to the same display-name id", () => {
    const config: ChannelsConfig = {
      Destiny: { kick: "destiny" },
      DESTINY: { twitch: "destinytv" },
    };
    expect(() => buildStreamers(config)).toThrow(/Duplicate streamer/);
  });

  it("applies pushoverToken from the entry", () => {
    const config: ChannelsConfig = {
      Destiny: { kick: "destiny", pushoverToken: "tok-abc" },
    };
    expect(buildStreamers(config)[0].pushoverToken).toBe("tok-abc");
  });

  it("applies liveNotifications from the entry", () => {
    const config: ChannelsConfig = {
      Destiny: { kick: "destiny", liveNotifications: false },
    };
    expect(buildStreamers(config)[0].liveNotifications).toBe(false);
  });

  it("leaves liveNotifications undefined without an override", () => {
    const config: ChannelsConfig = { Destiny: { kick: "destiny" } };
    expect(buildStreamers(config)[0].liveNotifications).toBeUndefined();
  });

  it("defaults tier to primary without an override", () => {
    const config: ChannelsConfig = { Destiny: { kick: "destiny" } };
    expect(buildStreamers(config)[0].tier).toBe("primary");
  });

  it("applies tier from the entry", () => {
    const config: ChannelsConfig = {
      Destiny: { kick: "destiny", tier: "background" },
    };
    expect(buildStreamers(config)[0].tier).toBe("background");
  });
});

describe("dropPlatformBindings", () => {
  function streamer(overrides: Partial<Streamer> = {}): Streamer {
    return {
      id: "s",
      displayName: "S",
      bindings: [],
      tier: "primary",
      ...overrides,
    };
  }

  it("removes only the target platform's bindings, keeping others", () => {
    const streamers = [
      streamer({
        bindings: [
          { platform: Platform.YouTube, username: "@a" },
          { platform: Platform.Kick, username: "a" },
        ],
      }),
    ];
    const { streamers: result, droppedAny } = dropPlatformBindings(
      streamers,
      Platform.Kick,
    );
    expect(droppedAny).toBe(true);
    expect(result).toHaveLength(1);
    expect(result[0].bindings).toEqual([
      { platform: Platform.YouTube, username: "@a" },
    ]);
  });

  it("drops a streamer entirely when its only binding is the target platform", () => {
    const streamers = [
      streamer({
        id: "kickonly",
        bindings: [{ platform: Platform.Kick, username: "a" }],
      }),
      streamer({
        id: "mixed",
        bindings: [
          { platform: Platform.YouTube, username: "@b" },
          { platform: Platform.Kick, username: "b" },
        ],
      }),
    ];
    const { streamers: result, droppedAny } = dropPlatformBindings(
      streamers,
      Platform.Kick,
    );
    expect(droppedAny).toBe(true);
    expect(result.map((s) => s.id)).toEqual(["mixed"]);
  });

  it("reports droppedAny: false and returns streamers unchanged when the platform isn't present", () => {
    const streamers = [
      streamer({ bindings: [{ platform: Platform.YouTube, username: "@a" }] }),
    ];
    const { streamers: result, droppedAny } = dropPlatformBindings(
      streamers,
      Platform.Kick,
    );
    expect(droppedAny).toBe(false);
    expect(result).toEqual(streamers);
  });
});

describe("isStreamerDue", () => {
  it("polls primary streamers on every tick", () => {
    for (const tick of [0, 1, 2, 3, 4, 5]) {
      expect(isStreamerDue("primary", tick)).toBe(true);
    }
  });

  it("polls background streamers on the startup tick", () => {
    expect(isStreamerDue("background", 0)).toBe(true);
  });

  it("skips background streamers between poll-factor ticks", () => {
    expect(isStreamerDue("background", 1)).toBe(false);
    expect(isStreamerDue("background", 2)).toBe(false);
    expect(isStreamerDue("background", BACKGROUND_POLL_FACTOR + 1)).toBe(false);
  });

  it("polls background streamers on every poll-factor-th tick", () => {
    expect(isStreamerDue("background", BACKGROUND_POLL_FACTOR)).toBe(true);
    expect(isStreamerDue("background", BACKGROUND_POLL_FACTOR * 2)).toBe(true);
  });
});
