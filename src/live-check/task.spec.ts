import { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { describe, expect, it } from "@effect/vitest";
import { Cause, Effect, Exit, Fiber } from "effect";
import { TestClock } from "effect/testing";
import { afterEach, beforeEach, vi } from "vitest";
import type { DggFeed } from "./dgg.js";
import {
  ProfileIdentityLinkEntity,
  rememberProfileIdentityLinkEffect,
} from "./identityLinks.js";
import type { LivestreamIntelligenceObserver } from "./intelligence/service.js";
import {
  getStreamerStatusEffect,
  StreamerStatusEntity,
  upsertStreamerStatusEffect,
} from "./persistence.js";
import { LiveStatus, Platform, platformConfigs } from "./platforms/index.js";
import type { Streamer } from "./streamers.js";
import LiveCheckTask from "./task.js";
import { provideTest, runTest, testRuntime } from "./testRuntime.js";
import { getStreamSessions, StreamSessionsEntity } from "./sessions.js";

vi.mock("@micthiesen/mitools/pushover", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@micthiesen/mitools/pushover")>()),
  notify: vi.fn(),
}));

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
  beforeEach(() =>
    vi
      .mocked(notify)
      .mockReset()
      .mockReturnValue(Effect.void as never),
  );
  afterEach(async () => {
    await runTest(ProfileIdentityLinkEntity.deleteAll());
    await runTest(StreamerStatusEntity.deleteAll());
    await runTest(StreamSessionsEntity.deleteAll());
  });

  it("refreshes and polls DGG streams only on the background cadence", async () => {
    const sharedStreamers: Streamer[] = [];
    const snapshots = [feed("first"), feed()];
    let fetches = 0;
    const task = new LiveCheckTask(
      sharedStreamers,
      Logger.named("DggTaskTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        learnIdentity: async () => undefined,
        fetchFeed: async () => snapshots[fetches++] ?? feed(),
      },
    );

    await runTest(task.run);
    expect(fetches).toBe(1);
    expect(sharedStreamers.map((streamer) => streamer.id)).toEqual([
      "dgg:twitch:first",
    ]);

    await runTest(task.run);
    await runTest(task.run);
    expect(fetches).toBe(1);
    expect(sharedStreamers).toHaveLength(1);

    await runTest(task.run);
    expect(fetches).toBe(2);
    expect(sharedStreamers).toEqual([]);
    expect(notify).not.toHaveBeenCalled();
  });

  it("retains the last selection but observes it as unknown after a refresh failure", async () => {
    const sharedStreamers: Streamer[] = [];
    let fetches = 0;
    const task = new LiveCheckTask(
      sharedStreamers,
      Logger.named("DggTaskFailureTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        learnIdentity: async () => undefined,
        fetchFeed: async () => {
          fetches += 1;
          if (fetches === 1) return feed("retained");
          throw new Error("DGG unavailable");
        },
      },
    );

    await runTest(task.run);
    await runTest(task.run);
    await runTest(task.run);
    await runTest(task.run);

    expect(sharedStreamers.map((streamer) => streamer.id)).toEqual([
      "dgg:twitch:retained",
    ]);
    expect(notify).not.toHaveBeenCalled();
  });

  it("propagates interruption while refreshing DGG streams", async () => {
    const fetchFeed = vi.fn(() => Effect.never);
    const task = new LiveCheckTask([], Logger.named("DggInterruptTest"), undefined, {
      topEmbeds: 1,
      availablePlatforms: new Set(Object.values(Platform)),
      fetchFeed,
    });
    const fiber = testRuntime.runFork(task.runEffect());
    await vi.waitFor(() => expect(fetchFeed).toHaveBeenCalledTimes(1));

    await Effect.runPromise(Fiber.interrupt(fiber));
    const exit = await Effect.runPromise(Fiber.await(fiber));

    expect(Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)).toBe(true);
  });

  it.effect(
    "propagates DGG persistence failures instead of treating them as feed outages",
    () =>
      Effect.gen(function* () {
        const getAll = vi
          .spyOn(ProfileIdentityLinkEntity, "getAll")
          .mockReturnValueOnce(
            Effect.fail(new Error("identity database unavailable")) as never,
          );
        const task = new LiveCheckTask(
          [],
          Logger.named("DggPersistenceTest"),
          undefined,
          {
            topEmbeds: 1,
            availablePlatforms: new Set(Object.values(Platform)),
            fetchFeed: () => Effect.succeed(feed()),
          },
        );

        try {
          const exit = yield* Effect.exit(task.runEffect());
          expect(Exit.isFailure(exit)).toBe(true);
          if (Exit.isFailure(exit)) {
            expect(String(exit.cause)).toContain("PersistenceError");
            expect(String(exit.cause)).toContain("identity database unavailable");
          }
        } finally {
          getAll.mockRestore();
        }
      }).pipe(provideTest),
  );

  it("schedules intelligence work only after every due streamer was observed", async () => {
    const observer = {
      observeLive: vi.fn(() => Effect.void),
      observeOffline: vi.fn(() => Effect.void),
      afterTick: vi.fn(() => Effect.void),
      close: vi.fn(() => Effect.void),
    } satisfies LivestreamIntelligenceObserver;
    const task = new LiveCheckTask(
      [],
      Logger.named("DggIntelligenceSchedulingTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        learnIdentity: async () => undefined,
        fetchFeed: async () => feed("voice-target"),
      },
      observer,
    );

    await runTest(task.run);

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
      .mockReturnValue(
        Effect.succeed({
          status: LiveStatus.Live,
          title: "Configured live",
          viewerCount: 0,
        }),
      );
    const snapshots = [feed("configured"), feed()];
    let dggFetches = 0;
    const task = new LiveCheckTask(
      sharedStreamers,
      Logger.named("DggConfiguredMergeTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        learnIdentity: async () => undefined,
        fetchFeed: async () => snapshots[dggFetches++] ?? feed(),
      },
    );

    try {
      await runTest(task.run);
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

      await runTest(task.run);
      await runTest(task.run);
      await runTest(task.run);
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

  it("learns a DGG profile identity and keeps per-platform observations", async () => {
    const youtube = { platform: Platform.YouTube, username: "@imreallyimportant" };
    const kick = { platform: Platform.Kick, username: "imreallyimportant" };
    const sharedStreamers: Streamer[] = [
      {
        id: "iri",
        displayName: "IRI",
        bindings: [youtube],
        tier: "background",
      },
    ];
    const youtubeFetch = vi
      .spyOn(platformConfigs[Platform.YouTube], "fetchLiveStatus")
      .mockReturnValue(
        Effect.succeed({
          status: LiveStatus.Live,
          title: "Election night",
          viewerCount: 427,
        }),
      );
    const task = new LiveCheckTask(
      sharedStreamers,
      Logger.named("DggProfileIdentityTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        fetchFeed: async () => ({
          destinyLive: false,
          hosting: null,
          embeds: [
            {
              platform: "kick",
              id: "imreallyimportant",
              count: 61,
              mediaItem: {
                identifier: {
                  platform: "kick",
                  mediaId: "imreallyimportant",
                },
                metadata: {
                  displayName: "imreallyimportant",
                  title: "Election night",
                  live: true,
                  viewers: 475,
                },
              },
            },
          ],
        }),
        learnIdentity: ({ source }) =>
          rememberProfileIdentityLinkEffect({ source, target: youtube }),
      },
    );

    try {
      await runTest(task.run);
      expect(sharedStreamers).toHaveLength(1);
      expect(sharedStreamers[0]?.bindings).toEqual([
        youtube,
        expect.objectContaining(kick),
      ]);
      expect(await runTest(getStreamerStatusEffect("iri"))).toMatchObject({
        isLive: true,
        viewerCount: 902,
        sources: [
          expect.objectContaining({ platform: Platform.YouTube, viewerCount: 427 }),
          expect.objectContaining({ platform: Platform.Kick, viewerCount: 475 }),
        ],
      });
    } finally {
      youtubeFetch.mockRestore();
    }
  });

  it("removes a stale profile identity when direct ownership evidence disappears", async () => {
    const youtube = { platform: Platform.YouTube, username: "@iri" };
    const kick = { platform: Platform.Kick, username: "iri" };
    await runTest(
      rememberProfileIdentityLinkEffect({ source: kick, target: youtube, now: 0 }),
    );
    const sharedStreamers: Streamer[] = [
      {
        id: "iri",
        displayName: "IRI",
        bindings: [youtube],
        tier: "background",
      },
    ];
    const youtubeFetch = vi
      .spyOn(platformConfigs[Platform.YouTube], "fetchLiveStatus")
      .mockReturnValue(Effect.succeed({ status: LiveStatus.Offline }));
    const task = new LiveCheckTask(
      sharedStreamers,
      Logger.named("DggStaleProfileIdentityTest"),
      undefined,
      {
        topEmbeds: 1,
        availablePlatforms: new Set(Object.values(Platform)),
        fetchFeed: async () => ({
          destinyLive: false,
          hosting: null,
          embeds: [
            {
              platform: "kick",
              id: "iri",
              count: 10,
              mediaItem: {
                identifier: { platform: "kick", mediaId: "iri" },
                metadata: {
                  displayName: "Different Display Name",
                  title: "No longer linked",
                  live: true,
                  viewers: 20,
                },
              },
            },
          ],
        }),
        learnIdentity: async () => undefined,
      },
    );

    try {
      await runTest(task.run);
      expect(await runTest(ProfileIdentityLinkEntity.getAll())).toEqual([]);
      expect(sharedStreamers.map((streamer) => streamer.id)).toEqual([
        "iri",
        "dgg:kick:iri",
      ]);
    } finally {
      youtubeFetch.mockRestore();
    }
  });

  it("persists a live edge before notification failure so the alert is not repeated", async () => {
    const streamer: Streamer = {
      id: "durable-live",
      displayName: "Durable Live",
      bindings: [{ platform: Platform.Twitch, username: "durable" }],
      tier: "primary",
    };
    const connector = vi
      .spyOn(platformConfigs[Platform.Twitch], "fetchLiveStatus")
      .mockReturnValue(
        Effect.succeed({
          status: LiveStatus.Live,
          title: "Already recorded",
        }),
      );
    vi.mocked(notify).mockReturnValueOnce(
      Effect.fail(new Error("Pushover unavailable")) as never,
    );
    const task = new LiveCheckTask([streamer], Logger.named("DurableLiveTest"));

    await expect(runTest(task.run)).rejects.toThrow("Pushover unavailable");
    expect(await runTest(getStreamerStatusEffect(streamer.id))).toMatchObject({
      isLive: true,
    });
    await expect(runTest(task.run)).resolves.toBeUndefined();
    expect(notify).toHaveBeenCalledTimes(1);
    connector.mockRestore();
  });

  it("retries an offline alert before durably closing the session", async () => {
    const streamer: Streamer = {
      id: "durable-offline",
      displayName: "Durable Offline",
      bindings: [{ platform: Platform.Twitch, username: "durable" }],
      tier: "primary",
    };
    await runTest(
      upsertStreamerStatusEffect({
        streamerId: streamer.id,
        isLive: true,
        primary: streamer.bindings[0]!,
        primaryTitle: "Session",
        startedAt: new Date(Date.now() - 60_000),
        maxViewerCount: 10,
      }),
    );
    const connector = vi
      .spyOn(platformConfigs[Platform.Twitch], "fetchLiveStatus")
      .mockReturnValue(
        Effect.succeed({
          status: LiveStatus.Offline,
        }),
      );
    vi.mocked(notify).mockReturnValueOnce(
      Effect.fail(new Error("Pushover unavailable")) as never,
    );
    const task = new LiveCheckTask([streamer], Logger.named("DurableOfflineTest"));

    await expect(runTest(task.run)).rejects.toThrow("Pushover unavailable");
    expect(await runTest(getStreamerStatusEffect(streamer.id))).toMatchObject({
      isLive: true,
    });
    expect((await runTest(getStreamSessions(streamer.id))).sessions).toHaveLength(0);
    await expect(runTest(task.run)).resolves.toBeUndefined();
    expect(await runTest(getStreamerStatusEffect(streamer.id))).toMatchObject({
      isLive: false,
    });
    expect((await runTest(getStreamSessions(streamer.id))).sessions).toHaveLength(1);
    expect(notify).toHaveBeenCalledTimes(2);
    connector.mockRestore();
  });

  it.effect("uses the Effect clock for transition timestamps", () =>
    Effect.gen(function* () {
      yield* TestClock.adjust("2 minutes");
      const streamer: Streamer = {
        id: "clocked-live",
        displayName: "Clocked Live",
        bindings: [{ platform: Platform.Twitch, username: "clocked" }],
        tier: "primary",
      };
      const connector = vi
        .spyOn(platformConfigs[Platform.Twitch], "fetchLiveStatus")
        .mockReturnValue(
          Effect.succeed({
            status: LiveStatus.Live,
            title: "Virtual time",
          }),
        );

      try {
        const task = new LiveCheckTask([streamer], Logger.named("ClockedLiveTest"));
        yield* task.runEffect();

        const status = yield* getStreamerStatusEffect(streamer.id);
        expect(status.isLive).toBe(true);
        if (status.isLive) expect(new Date(status.startedAt).getTime()).toBe(120_000);
      } finally {
        connector.mockRestore();
      }
    }).pipe(provideTest),
  );

  it.effect("returns streamer persistence failures in the typed error channel", () =>
    Effect.gen(function* () {
      const streamer: Streamer = {
        id: "persistence-failure",
        displayName: "Persistence Failure",
        bindings: [{ platform: Platform.Twitch, username: "broken" }],
        tier: "primary",
      };
      const connector = vi
        .spyOn(platformConfigs[Platform.Twitch], "fetchLiveStatus")
        .mockReturnValue(
          Effect.succeed({ status: LiveStatus.Live, title: "Cannot persist" }),
        );
      const upsert = vi
        .spyOn(StreamerStatusEntity, "upsert")
        .mockReturnValueOnce(
          Effect.fail(new Error("streamer database unavailable")) as never,
        );

      try {
        const task = new LiveCheckTask(
          [streamer],
          Logger.named("PersistenceFailureTest"),
        );
        const exit = yield* Effect.exit(task.runEffect());

        expect(Exit.isFailure(exit)).toBe(true);
        if (Exit.isFailure(exit)) {
          expect(String(exit.cause)).toContain("PersistenceError");
          expect(String(exit.cause)).toContain("streamer database unavailable");
        }
      } finally {
        upsert.mockRestore();
        connector.mockRestore();
      }
    }).pipe(provideTest),
  );
});
