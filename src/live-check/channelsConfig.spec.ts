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
    expect(loadChannelsConfig()).toEqual({});
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
      Destiny: { youtube: "@destiny", kick: "destiny", pushoverToken: "tok" },
      Hutch: { youtube: "@hutch", liveNotifications: false },
      Jerma: { twitch: "jerma985", liveNotifications: false },
    });
  });

  it("accepts a string[] of usernames on a platform field", () => {
    withConfig(JSON.stringify({ Destiny: { twitch: ["destiny", "destinyalt"] } }));
    expect(loadChannelsConfig()).toEqual({
      Destiny: { twitch: ["destiny", "destinyalt"] },
    });
  });

  it("accepts an entry with a tier field", () => {
    withConfig(JSON.stringify({ Destiny: { kick: "destiny", tier: "background" } }));
    expect(loadChannelsConfig()).toEqual({
      Destiny: { kick: "destiny", tier: "background" },
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
      Destiny: { kick: "destiny", liveNotifications: false },
    });
  });

  it("allows tier: primary alongside an explicit liveNotifications", () => {
    withConfig(
      JSON.stringify({
        Destiny: { kick: "destiny", tier: "primary", liveNotifications: false },
      }),
    );
    expect(loadChannelsConfig()).toEqual({
      Destiny: { kick: "destiny", tier: "primary", liveNotifications: false },
    });
  });
});
