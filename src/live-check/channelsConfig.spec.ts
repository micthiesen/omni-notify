import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { loadChannelsConfig } from "./channelsConfig.js";

let dir: string;
let originalEnv: string | undefined;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "omni-channels-config-"));
  originalEnv = process.env.CHANNELS_CONFIG_PATH;
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
  if (originalEnv === undefined) {
    process.env.CHANNELS_CONFIG_PATH = undefined;
    delete process.env.CHANNELS_CONFIG_PATH;
  } else {
    process.env.CHANNELS_CONFIG_PATH = originalEnv;
  }
});

function withConfig(content: string): void {
  const path = join(dir, "channels.json");
  writeFileSync(path, content);
  process.env.CHANNELS_CONFIG_PATH = path;
}

describe("loadChannelsConfig", () => {
  it("returns {} when the file does not exist", () => {
    process.env.CHANNELS_CONFIG_PATH = join(dir, "does-not-exist.json");
    expect(loadChannelsConfig()).toEqual({ channels: {}, dggTopEmbeds: 0 });
  });

  it("parses a full production-shaped config", () => {
    withConfig(
      JSON.stringify({
        Destiny: { youtube: "@destiny", kick: "destiny", pushoverToken: "tok" },
        Hutch: { youtube: "@hutch", liveNotifications: false },
        Jerma: { twitch: "jerma985", liveNotifications: false },
      }),
    );
    expect(loadChannelsConfig()).toEqual({
      channels: {
        Destiny: { youtube: "@destiny", kick: "destiny", pushoverToken: "tok" },
        Hutch: { youtube: "@hutch", liveNotifications: false },
        Jerma: { twitch: "jerma985", liveNotifications: false },
      },
      dggTopEmbeds: 0,
    });
  });

  it("accepts a string[] of usernames on a platform field", () => {
    withConfig(JSON.stringify({ Destiny: { twitch: ["destiny", "destinyalt"] } }));
    expect(loadChannelsConfig()).toEqual({
      channels: { Destiny: { twitch: ["destiny", "destinyalt"] } },
      dggTopEmbeds: 0,
    });
  });

  it("accepts an entry with a tier field", () => {
    withConfig(JSON.stringify({ Destiny: { kick: "destiny", tier: "background" } }));
    expect(loadChannelsConfig()).toEqual({
      channels: { Destiny: { kick: "destiny", tier: "background" } },
      dggTopEmbeds: 0,
    });
  });

  it("throws on malformed JSON", () => {
    withConfig("{ not valid json");
    expect(() => loadChannelsConfig()).toThrow(/Failed to parse channels config/);
  });

  it("throws on a schema violation (bad tier enum value)", () => {
    withConfig(JSON.stringify({ Destiny: { kick: "destiny", tier: "vip" } }));
    expect(() => loadChannelsConfig()).toThrow(/Invalid channels config/);
  });

  it("rejects an unknown key on an entry", () => {
    withConfig(JSON.stringify({ Destiny: { kick: "destiny", twich: "typo" } }));
    expect(() => loadChannelsConfig()).toThrow(/Invalid channels config/);
  });

  it("rejects an entry with no platform fields", () => {
    withConfig(JSON.stringify({ Destiny: { pushoverToken: "tok" } }));
    expect(() => loadChannelsConfig()).toThrow(/at least one platform/);
  });

  it("rejects an entry with an empty username string", () => {
    withConfig(JSON.stringify({ Destiny: { kick: "" } }));
    expect(() => loadChannelsConfig()).toThrow(/Invalid channels config/);
  });

  it("rejects an entry with an empty usernames array", () => {
    withConfig(JSON.stringify({ Destiny: { twitch: [] } }));
    expect(() => loadChannelsConfig()).toThrow(/Invalid channels config/);
  });

  it("rejects a blank display-name key", () => {
    withConfig(JSON.stringify({ "  ": { twitch: "someone" } }));
    expect(() => loadChannelsConfig()).toThrow(/display name must not be blank/);
  });

  it("throws when tier: background is combined with liveNotifications: true", () => {
    withConfig(
      JSON.stringify({
        Destiny: { kick: "destiny", tier: "background", liveNotifications: true },
      }),
    );
    expect(() => loadChannelsConfig()).toThrow(/contradicts the background tier/);
  });

  it("throws when tier: background is combined with liveNotifications: false", () => {
    withConfig(
      JSON.stringify({
        Destiny: { kick: "destiny", tier: "background", liveNotifications: false },
      }),
    );
    expect(() => loadChannelsConfig()).toThrow(/redundant/);
  });

  it("allows liveNotifications: false without a tier", () => {
    withConfig(
      JSON.stringify({ Destiny: { kick: "destiny", liveNotifications: false } }),
    );
    expect(loadChannelsConfig()).toEqual({
      channels: { Destiny: { kick: "destiny", liveNotifications: false } },
      dggTopEmbeds: 0,
    });
  });

  it("allows tier: primary alongside an explicit liveNotifications", () => {
    withConfig(
      JSON.stringify({
        Destiny: { kick: "destiny", tier: "primary", liveNotifications: false },
      }),
    );
    expect(loadChannelsConfig()).toEqual({
      channels: {
        Destiny: { kick: "destiny", tier: "primary", liveNotifications: false },
      },
      dggTopEmbeds: 0,
    });
  });

  it("accepts a Destiny.gg top-embeds count alongside channels", () => {
    withConfig(JSON.stringify({ dggTopEmbeds: 3, Destiny: { kick: "destiny" } }));
    expect(loadChannelsConfig()).toEqual({
      channels: { Destiny: { kick: "destiny" } },
      dggTopEmbeds: 3,
    });
  });

  it("rejects an invalid Destiny.gg top-embeds count", () => {
    withConfig(JSON.stringify({ dggTopEmbeds: -1 }));
    expect(() => loadChannelsConfig()).toThrow(/dggTopEmbeds/);
  });
});
