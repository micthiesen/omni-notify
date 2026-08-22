import { describe, expect, it } from "vitest";

import {
  type DggFeed,
  type DggWebSocketFactory,
  fetchDggFeed,
  selectDggStreams,
} from "./dgg.js";
import { Platform } from "./platforms/index.js";
import type { Streamer } from "./streamers.js";

function embed({
  platform,
  id,
  displayName = id,
  count = 10,
  live = true,
}: {
  platform: string;
  id: string;
  displayName?: string;
  count?: number;
  live?: boolean;
}) {
  return {
    platform,
    id,
    count,
    mediaItem: {
      identifier: { platform, mediaId: id },
      metadata: {
        previewUrl: `https://images.test/${id}.jpg`,
        displayName,
        title: `${displayName}'s title`,
        createdDate: "2026-08-22T20:24:55+00:00",
        live,
        viewers: 735,
      },
    },
  };
}

describe("selectDggStreams", () => {
  it("selects nothing when the feature is disabled", () => {
    expect(
      selectDggStreams({
        feed: {
          destinyLive: false,
          hosting: null,
          embeds: [embed({ platform: "kick", id: "ignored" })],
        },
        limit: 0,
        configuredStreamers: [],
        availablePlatforms: new Set(Object.values(Platform)),
      }),
    ).toEqual([]);
  });

  it("uses the only slot for an active host", () => {
    const selected = selectDggStreams({
      feed: {
        destinyLive: false,
        hosting: {
          platform: "twitch",
          id: "host",
          displayName: "Host",
        },
        embeds: [embed({ platform: "kick", id: "top", count: 999 })],
      },
      limit: 1,
      configuredStreamers: [],
      availablePlatforms: new Set(Object.values(Platform)),
    });

    expect(selected).toHaveLength(1);
    expect(selected[0]?.streamer.id).toBe("dgg:twitch:host");
    expect(selected[0]?.hosted).toBe(true);
  });

  it("deduplicates sources and fills past configured high-ranked embeds", () => {
    const selected = selectDggStreams({
      feed: {
        destinyLive: false,
        hosting: null,
        embeds: [
          embed({ platform: "twitch", id: "configured", count: 500 }),
          embed({ platform: "kick", id: "duplicate", count: 400 }),
          embed({ platform: "kick", id: "duplicate", count: 300 }),
          embed({ platform: "youtube", id: "filled", count: 200 }),
        ],
      },
      limit: 2,
      configuredStreamers: [
        {
          id: "configured",
          displayName: "Configured",
          bindings: [{ platform: Platform.Twitch, username: "configured" }],
          tier: "primary",
        },
      ],
      availablePlatforms: new Set(Object.values(Platform)),
    });

    expect(selected.map((entry) => entry.streamer.id)).toEqual([
      "dgg:kick:duplicate",
      "dgg:youtube:filled",
    ]);
  });

  it("prefers a host within the limit and retains DGG metadata", () => {
    const feed: DggFeed = {
      destinyLive: false,
      hosting: {
        platform: "twitch",
        id: "host_channel",
        displayName: "Host Channel",
        title: "A hosted stream",
        preview: "https://images.test/host.jpg",
      },
      embeds: [
        embed({ platform: "kick", id: "first", count: 99 }),
        embed({ platform: "twitch", id: "host_channel", count: 50 }),
        embed({ platform: "youtube", id: "video-id", count: 40 }),
      ],
    };

    const selected = selectDggStreams({
      feed,
      limit: 3,
      configuredStreamers: [],
      availablePlatforms: new Set(Object.values(Platform)),
    });

    expect(selected.map((entry) => entry.streamer.id)).toEqual([
      "dgg:twitch:host_channel",
      "dgg:kick:first",
      "dgg:youtube:video-id",
    ]);
    expect(selected[0]).toMatchObject({
      hosted: true,
      previewUrl: "https://images.test/host.jpg",
      url: "https://www.twitch.tv/host_channel",
      status: {
        title: "A hosted stream",
        viewerCount: 735,
        startedAt: "2026-08-22T20:24:55+00:00",
      },
      streamer: {
        dgg: { hosted: true, viewers: 50 },
        bindings: [{ urlOverride: "https://www.twitch.tv/host_channel" }],
      },
    });
    expect(selected[1]).toMatchObject({
      hosted: false,
      embedCount: 99,
      status: {
        title: "first's title",
        viewerCount: 735,
        startedAt: "2026-08-22T20:24:55+00:00",
      },
      streamer: { tier: "background", dgg: { hosted: false, viewers: 99 } },
    });
    expect(selected[2]?.url).toBe("https://www.youtube.com/watch?v=video-id");
  });

  it("filters configured collisions, unusable platforms, and non-live embeds", () => {
    const configured: Streamer[] = [
      {
        id: "known-binding",
        displayName: "Someone Else",
        bindings: [{ platform: Platform.Twitch, username: "KnownChannel" }],
        tier: "primary",
      },
      {
        id: "same-name",
        displayName: "Display Collision",
        bindings: [{ platform: Platform.Kick, username: "different" }],
        tier: "primary",
      },
    ];
    const feed: DggFeed = {
      destinyLive: false,
      hosting: null,
      embeds: [
        embed({ platform: "twitch", id: "knownchannel" }),
        embed({ platform: "kick", id: "new-id", displayName: "display collision" }),
        embed({ platform: "youtube", id: "unavailable" }),
        embed({ platform: "rumble", id: "unsupported" }),
        embed({ platform: "kick", id: "offline", live: false }),
        embed({ platform: "kick", id: "kept" }),
      ],
    };

    const selected = selectDggStreams({
      feed,
      limit: 10,
      configuredStreamers: configured,
      availablePlatforms: new Set([Platform.Twitch, Platform.Kick]),
    });

    expect(selected.map((entry) => entry.streamer.id)).toEqual(["dgg:kick:kept"]);
  });

  it("suppresses hosting when Destiny is live", () => {
    const selected = selectDggStreams({
      feed: {
        destinyLive: true,
        hosting: {
          platform: "twitch",
          id: "stale-host",
          displayName: "Stale Host",
        },
        embeds: [embed({ platform: "kick", id: "still-embedded" })],
      },
      limit: 1,
      configuredStreamers: [],
      availablePlatforms: new Set(Object.values(Platform)),
    });

    expect(selected[0]?.streamer.id).toBe("dgg:kick:still-embedded");
    expect(selected[0]?.hosted).toBe(false);
  });
});

class FakeSocket {
  listeners = new Map<string, Array<(event: { data: unknown }) => void>>();
  closed = false;

  addEventListener(type: string, listener: (event: { data: unknown }) => void) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  close() {
    this.closed = true;
  }

  message(value: unknown) {
    for (const listener of this.listeners.get("message") ?? []) {
      listener({ data: JSON.stringify(value) });
    }
  }
}

describe("fetchDggFeed", () => {
  it("rejects when a complete snapshot does not arrive before the timeout", async () => {
    const socket = new FakeSocket();
    await expect(
      fetchDggFeed({
        timeoutMs: 5,
        createSocket: (() => socket) as DggWebSocketFactory,
      }),
    ).rejects.toThrow("Timed out waiting for DGG live snapshot");
    expect(socket.closed).toBe(true);
  });

  it("waits for the complete snapshot in any order and suppresses stale hosting", async () => {
    const socket = new FakeSocket();
    const promise = fetchDggFeed({
      timeoutMs: 1_000,
      createSocket: (() => socket) as DggWebSocketFactory,
    });

    socket.message({
      type: "dggApi:hosting",
      data: {
        platform: "twitch",
        id: "host",
        displayName: "Host",
        title: "Hosted title",
        preview: null,
      },
    });
    socket.message({ type: "unrelated", data: {} });
    socket.message({
      type: "dggApi:embeds",
      data: [embed({ platform: "kick", id: "x" })],
    });
    socket.message({
      type: "dggApi:streamInfo",
      data: { streams: { twitch: null, youtube: { live: true, extra: "allowed" } } },
    });

    await expect(promise).resolves.toMatchObject({
      destinyLive: true,
      hosting: null,
      embeds: [{ id: "x" }],
    });
    expect(socket.closed).toBe(true);
  });

  it("rejects malformed relevant payloads instead of returning a partial feed", async () => {
    const socket = new FakeSocket();
    const promise = fetchDggFeed({
      timeoutMs: 1_000,
      createSocket: (() => socket) as DggWebSocketFactory,
    });

    socket.message({ type: "dggApi:embeds", data: [{ nope: true }] });

    await expect(promise).rejects.toThrow("Invalid dggApi:embeds payload");
    expect(socket.closed).toBe(true);
  });
});
