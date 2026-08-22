import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { beforeEach, describe, expect, it, vi } from "vitest";
import appConfig from "../utils/config.js";
import type { DggFeed } from "./dgg.js";
import { Platform } from "./platforms/index.js";
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
});
