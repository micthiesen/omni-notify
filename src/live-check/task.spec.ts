import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { beforeEach, describe, expect, it, vi } from "vitest";
import appConfig from "../utils/config.js";
import type { DggFeed } from "./dgg.js";
import type { LivestreamIntelligenceObserver } from "./intelligence/service.js";
import { LiveStatus, Platform, platformConfigs } from "./platforms/index.js";
import type { Streamer } from "./streamers.js";
import LiveCheckTask from "./task.js";

vi.mock("@micthiesen/mitools/pushover", () => ({ notify: vi.fn() }));

Injector.configure({
  config: { ...appConfig, DB_NAME: `/tmp/omni-dgg-task-${process.pid}.db` },
});

function feed(id?: string): DggFeed {
  return {
    destinyLive: false,
    hosting: null,
    embeds: id
      ? [
          {
            platform: "twitch",
            id,
            count: 12,
            mediaItem: {
              identifier: { platform: "twitch", mediaId: id },
              metadata: {
                displayName: id,
                title: `${id} live`,
                live: true,
                viewers: 0,
              },
            },
          },
        ]
      : [],
  };
}

describe("LiveCheckTask DGG discovery", () => {
  beforeEach(() => vi.mocked(notify).mockClear());

  it("refreshes and polls DGG streams only on the background cadence", async () => {
    const sharedStreamers: Streamer[] = [];
    const snapshots = [feed("first"), feed()];
    let fetches = 0;
    const task = new LiveCheckTask(
      sharedStreamers,
      new Logger("DggTaskTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        fetchFeed: async () => snapshots[fetches++] ?? feed(),
      },
    );

    await task.run();
    expect(fetches).toBe(1);
    expect(sharedStreamers.map((streamer) => streamer.id)).toEqual([
      "dgg:twitch:first",
    ]);

    await task.run();
    await task.run();
    expect(fetches).toBe(1);
    expect(sharedStreamers).toHaveLength(1);

    await task.run();
    expect(fetches).toBe(2);
    expect(sharedStreamers).toEqual([]);
    expect(notify).not.toHaveBeenCalled();
  });

  it("retains the last selection but observes it as unknown after a refresh failure", async () => {
    const sharedStreamers: Streamer[] = [];
    let fetches = 0;
    const task = new LiveCheckTask(
      sharedStreamers,
      new Logger("DggTaskFailureTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        fetchFeed: async () => {
          fetches += 1;
          if (fetches === 1) return feed("retained");
          throw new Error("DGG unavailable");
        },
      },
    );

    await task.run();
    await task.run();
    await task.run();
    await task.run();

    expect(sharedStreamers.map((streamer) => streamer.id)).toEqual([
      "dgg:twitch:retained",
    ]);
    expect(notify).not.toHaveBeenCalled();
  });

  it("schedules intelligence work only after every due streamer was observed", async () => {
    const observer = {
      observeLive: vi.fn(),
      observeOffline: vi.fn(),
      afterTick: vi.fn(),
      close: vi.fn(async () => undefined),
    } satisfies LivestreamIntelligenceObserver;
    const task = new LiveCheckTask(
      [],
      new Logger("DggIntelligenceSchedulingTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        fetchFeed: async () => feed("voice-target"),
      },
      observer,
    );

    await task.run();

    expect(observer.observeLive).toHaveBeenCalledTimes(1);
    expect(observer.afterTick).toHaveBeenCalledTimes(1);
    expect(observer.observeLive.mock.invocationCallOrder[0]).toBeLessThan(
      observer.afterTick.mock.invocationCallOrder[0] ?? 0,
    );
  });

  it("enriches an explicit primary streamer without duplicating or replacing its polling", async () => {
    const sharedStreamers: Streamer[] = [
      {
        id: "configured",
        displayName: "Configured",
        bindings: [{ platform: Platform.Twitch, username: "configured" }],
        tier: "primary",
      },
    ];
    const connector = vi
      .spyOn(platformConfigs[Platform.Twitch], "fetchLiveStatus")
      .mockResolvedValue({
        status: LiveStatus.Live,
        title: "Configured live",
        viewerCount: 0,
      });
    const snapshots = [feed("configured"), feed()];
    let dggFetches = 0;
    const task = new LiveCheckTask(
      sharedStreamers,
      new Logger("DggConfiguredMergeTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        fetchFeed: async () => snapshots[dggFetches++] ?? feed(),
      },
    );

    try {
      await task.run();
      expect(sharedStreamers).toHaveLength(1);
      expect(sharedStreamers[0]).toMatchObject({
        id: "configured",
        tier: "primary",
        bindings: [{ platform: Platform.Twitch, username: "configured" }],
        dgg: { hosted: false, viewers: 12 },
      });
      expect(dggFetches).toBe(1);
      expect(connector).toHaveBeenCalledTimes(1);
      expect(notify).toHaveBeenCalledTimes(1);

      await task.run();
      await task.run();
      await task.run();
      expect(dggFetches).toBe(2);
      expect(connector).toHaveBeenCalledTimes(4);
      expect(notify).toHaveBeenCalledTimes(1);
      expect(sharedStreamers).toEqual([
        {
          id: "configured",
          displayName: "Configured",
          bindings: [{ platform: Platform.Twitch, username: "configured" }],
          tier: "primary",
        },
      ]);
    } finally {
      connector.mockRestore();
    }
  });
});
