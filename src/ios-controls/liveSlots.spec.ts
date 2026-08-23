import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it } from "vitest";
import {
  StreamerStatusEntity,
  upsertStreamerStatus,
} from "../live-check/persistence.js";
import { Platform } from "../live-check/platforms/index.js";
import type { Streamer } from "../live-check/streamers.js";
import { buildLiveControlSlots, liveControlSlotHash } from "./liveSlots.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "ios-live-slots.spec.db",
  },
});

const streamers: Streamer[] = [
  {
    id: "alpha",
    displayName: "Alpha",
    bindings: [{ platform: Platform.Twitch, username: "alpha" }],
    tier: "primary",
  },
  {
    id: "beta",
    displayName: "Beta",
    bindings: [{ platform: Platform.YouTube, username: "beta" }],
    tier: "background",
  },
  {
    id: "gamma",
    displayName: "Gamma",
    bindings: [{ platform: Platform.Kick, username: "gamma" }],
    tier: "primary",
  },
];

afterEach(() => StreamerStatusEntity.deleteAll());

function live(id: string, platform: Platform, viewers: number): void {
  upsertStreamerStatus({
    streamerId: id,
    isLive: true,
    primary: { platform, username: id },
    primaryTitle: `${id} title`,
    startedAt: new Date("2026-08-18T12:00:00Z"),
    maxViewerCount: viewers + 10,
    viewerCount: viewers,
  });
}

describe("buildLiveControlSlots", () => {
  it("ranks primary channels before hotter background channels", () => {
    live("alpha", Platform.Twitch, 10);
    live("beta", Platform.YouTube, 10_000);
    live("gamma", Platform.Kick, 20);

    const slots = buildLiveControlSlots(streamers, "http://omni.boris", 123);
    expect(slots.map((slot) => slot.streamerId)).toEqual([
      "gamma",
      "alpha",
      "beta",
      null,
    ]);
    expect(slots[0]).toMatchObject({
      slot: 1,
      displayName: "Gamma",
      url: "https://kick.com/gamma",
      updatedAt: 123,
    });
    expect(slots[3]).toMatchObject({
      isLive: false,
      displayName: "Nobody Live",
      url: "http://omni.boris",
    });
  });

  it("ranks DGG-only channels by DGG viewers but returns platform viewers", () => {
    const dggStreamers: Streamer[] = [
      {
        id: "dgg:kick:large",
        displayName: "Large Platform Stream",
        bindings: [{ platform: Platform.Kick, username: "large" }],
        tier: "background",
        discoverySource: "dgg",
        dgg: { hosted: false, viewers: 10 },
      },
      {
        id: "configured-small",
        displayName: "Configured Small Stream",
        bindings: [{ platform: Platform.Kick, username: "configured-small" }],
        tier: "background",
        dgg: { hosted: true, viewers: 1 },
      },
    ];
    live("dgg:kick:large", Platform.Kick, 50_000);
    live("configured-small", Platform.Kick, 50);

    const slots = buildLiveControlSlots(dggStreamers, "http://omni.boris", 123);
    expect(slots.map((slot) => slot.streamerId)).toEqual([
      "configured-small",
      "dgg:kick:large",
      null,
      null,
    ]);
    expect(slots[1]?.viewerCount).toBe(50_000);
    expect(slots[1]).not.toHaveProperty("orderingViewerCount");
  });

  it("does not change the state hash when only updatedAt changes", () => {
    live("alpha", Platform.Twitch, 10);
    const first = buildLiveControlSlots(streamers, "http://omni.boris", 1)[0];
    const second = buildLiveControlSlots(streamers, "http://omni.boris", 2)[0];
    expect(liveControlSlotHash(first)).toBe(liveControlSlotHash(second));
  });

  it("does not push for viewer and uptime changes that leave the slot unchanged", () => {
    live("alpha", Platform.Twitch, 10);
    const first = buildLiveControlSlots(streamers, "http://omni.boris", 1)[0];
    upsertStreamerStatus({
      streamerId: "alpha",
      isLive: true,
      primary: { platform: Platform.Twitch, username: "alpha" },
      primaryTitle: "alpha title",
      startedAt: new Date("2026-08-18T13:00:00Z"),
      maxViewerCount: 999,
      viewerCount: 500,
    });
    const second = buildLiveControlSlots(streamers, "http://omni.boris", 2)[0];
    expect(liveControlSlotHash(first)).toBe(liveControlSlotHash(second));
  });
});
