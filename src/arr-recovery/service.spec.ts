import { Docstore } from "@micthiesen/mitools/docstore";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { Pushover, PushoverError } from "@micthiesen/mitools/pushover";
import { expect, layer } from "@effect/vitest";
import { Clock, Effect, Fiber, Layer, Option } from "effect";
import { TestClock } from "effect/testing";
import { vi } from "vitest";
import {
  acquireState,
  type RecoveryAction,
  RecoveryStateEntity,
  releaseState,
  saveState,
} from "./persistence.js";
import { observationFingerprint } from "./policy.js";
import {
  canReplace,
  runRecovery,
  STUCK_GRACE_MS,
  type RecoveryOptions,
} from "./service.js";
import { ArrRecoveryError } from "./types.js";
import type {
  ArrClient,
  Decision,
  Grab,
  ImportFile,
  QueueItem,
  Target,
} from "./types.js";

const NOW = 1_800_000_000_000;
const logger = Logger.named("ArrRecoveryTest");
const TestLayer = Layer.mergeAll(
  Docstore.layerMemory,
  Logger.layer({ level: LogLevel.ERROR, sinks: [] }),
  Pushover.layerNoop,
).pipe(Layer.orDie);

const target: Target = {
  id: 10,
  title: "Example Movie",
  year: 2026,
  monitored: true,
  hasFile: false,
  path: "/movies/Example Movie",
  episodeIds: [],
  episodes: [],
  alternateTitles: [],
};

const importFile: ImportFile = {
  id: 20,
  path: "/downloads/example/Example.Movie.2026.mkv",
  name: "Example.Movie.2026.mkv",
  size: 1_000,
  movieId: target.id,
  episodeIds: [],
  quality: {},
  rejections: [],
};

const grab: Grab = {
  downloadId: "download-1",
  sourceTitle: "Example.Movie.2026",
  movieId: target.id,
  eventType: "grabbed",
  date: "2026-09-12T00:00:00Z",
};

function stuckItem(overrides: Partial<QueueItem> = {}): QueueItem {
  return {
    id: 1,
    downloadId: "download-1",
    title: "Example.Movie.2026",
    status: "completed",
    trackedDownloadStatus: "warning",
    trackedDownloadState: "importBlocked",
    statusMessages: [
      {
        title: "Import failed",
        messages: ["Could not find a matching movie"],
      },
    ],
    size: 1_000,
    sizeleft: 0,
    outputPath: "/downloads/example",
    movieId: target.id,
    ...overrides,
  };
}

interface MockControls {
  client: ArrClient;
  queue: QueueItem[];
  target: Target;
  files: ImportFile[];
  grabs: Grab[];
  imported: boolean;
  removed: boolean;
  queueCall: ReturnType<typeof vi.fn>;
  preview: ReturnType<typeof vi.fn>;
  targetCall: ReturnType<typeof vi.fn>;
  history: ReturnType<typeof vi.fn>;
  importFiles: ReturnType<typeof vi.fn>;
  command: ReturnType<typeof vi.fn>;
  remove: ReturnType<typeof vi.fn>;
  search: ReturnType<typeof vi.fn>;
  verifyImported: ReturnType<typeof vi.fn>;
  verifyRemoved: ReturnType<typeof vi.fn>;
}

function mockClient(items: QueueItem[] = [stuckItem()]): MockControls {
  const controls: Omit<MockControls, "client"> = {
    queue: items,
    target,
    files: [],
    grabs: [grab],
    imported: false,
    removed: false,
    queueCall: vi.fn(),
    preview: vi.fn(),
    targetCall: vi.fn(),
    history: vi.fn(),
    importFiles: vi.fn(),
    command: vi.fn(),
    remove: vi.fn(),
    search: vi.fn(),
    verifyImported: vi.fn(),
    verifyRemoved: vi.fn(),
  };
  controls.queueCall.mockImplementation(() => Effect.succeed(controls.queue));
  controls.preview.mockImplementation(() => Effect.succeed(controls.files));
  controls.targetCall.mockImplementation(() => Effect.succeed(controls.target));
  controls.history.mockImplementation(() => Effect.succeed(controls.grabs));
  controls.importFiles.mockImplementation(() =>
    Effect.sync(() => {
      controls.imported = true;
      return 101;
    }),
  );
  controls.command.mockImplementation(() => Effect.succeed({ status: "completed" }));
  controls.remove.mockImplementation((id: number) =>
    Effect.sync(() => {
      controls.removed = true;
      controls.queue = controls.queue.filter((item) => item.id !== id);
    }),
  );
  controls.search.mockImplementation(() => Effect.succeed(202));
  controls.verifyImported.mockImplementation(() => Effect.succeed(controls.imported));
  controls.verifyRemoved.mockImplementation(() => Effect.succeed(controls.removed));

  const client = {
    kind: "radarr" as const,
    queue: controls.queueCall,
    preview: controls.preview,
    target: controls.targetCall,
    history: controls.history,
    importFiles: controls.importFiles,
    command: controls.command,
    remove: controls.remove,
    search: controls.search,
    verifyImported: controls.verifyImported,
    verifyRemoved: controls.verifyRemoved,
  } as ArrClient;
  return Object.assign(controls, { client });
}

const assess = (decision: Decision): NonNullable<RecoveryOptions["assess"]> =>
  vi.fn(() => Effect.succeed(decision));

const send = vi.fn(() => Effect.void);

function state() {
  return RecoveryStateEntity.get({ kind: "radarr" }).pipe(
    Effect.map(Option.getOrThrow),
  );
}

function seedMature(
  item: QueueItem,
  actions: RecoveryAction[] = [],
): Effect.Effect<void, unknown, Docstore> {
  return Effect.gen(function* () {
    const now = yield* Clock.currentTimeMillis;
    const owner = "test-seed";
    const stored = yield* acquireState("radarr", owner, now);
    if (!stored) throw new Error("test could not acquire recovery state");
    stored.observations[item.downloadId] = {
      fingerprint: observationFingerprint([item]),
      firstSeenAt: now - STUCK_GRACE_MS,
      lastSeenAt: now,
      observations: 2,
    };
    stored.actions = actions;
    yield* saveState(stored, owner, now);
    yield* releaseState("radarr", owner);
  });
}

function recovery(controls: MockControls, decision: Decision) {
  return runRecovery([controls.client], logger, {
    assess: assess(decision),
    send,
  });
}

layer(TestLayer)("Arr recovery service", (it) => {
  it.effect("retries a confirmed Pushover rejection without repeating the action", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const controls = mockClient();
      yield* seedMature(stuckItem());
      const rejection = new ArrRecoveryError({
        operation: "notify",
        cause: new PushoverError({
          status: 429,
          body: "rate limited",
          cause: undefined,
        }),
      });
      yield* Effect.result(
        runRecovery([controls.client], logger, {
          assess: assess({
            action: "remove",
            replace: false,
            reason: "duplicate",
            source: "llm",
          }),
          send: () => Effect.fail(rejection),
        }),
      );
      expect((yield* state()).actions[0].notification).toBe("pending");
      yield* recovery(controls, { action: "defer", reason: "unused", source: "llm" });
      expect((yield* state()).actions[0].notification).toBe("sent");
      expect(controls.remove).toHaveBeenCalledTimes(1);
    }),
  );

  it.effect("waits for a second unchanged observation spanning 15 minutes", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      send.mockClear();
      yield* TestClock.setTime(NOW);
      const controls = mockClient();
      const decision: Decision = {
        action: "remove",
        replace: false,
        reason: "duplicate",
        source: "llm",
      };

      yield* recovery(controls, decision);
      expect(controls.remove).not.toHaveBeenCalled();
      expect((yield* state()).actions).toHaveLength(0);

      yield* TestClock.adjust(STUCK_GRACE_MS);
      yield* recovery(controls, decision);
      expect(controls.remove).toHaveBeenCalledTimes(1);
      expect((yield* state()).actions[0]?.phase).toBe("done");
    }),
  );

  it.effect("restarts the grace period when queue progress changes", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const controls = mockClient();
      const decision: Decision = {
        action: "remove",
        replace: false,
        reason: "duplicate",
        source: "llm",
      };

      yield* recovery(controls, decision);
      yield* TestClock.adjust(STUCK_GRACE_MS);
      controls.queue = [
        stuckItem({
          statusMessages: [
            { title: "Import failed", messages: ["A different import failure"] },
          ],
        }),
      ];
      yield* recovery(controls, decision);
      expect(controls.remove).not.toHaveBeenCalled();

      yield* TestClock.adjust(STUCK_GRACE_MS);
      yield* recovery(controls, decision);
      expect(controls.remove).toHaveBeenCalledTimes(1);
    }),
  );

  it.effect("excludes normal active downloads", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const controls = mockClient([
        stuckItem({
          status: "downloading",
          trackedDownloadStatus: "ok",
          trackedDownloadState: "downloading",
          sizeleft: 500,
          statusMessages: [],
        }),
      ]);

      yield* recovery(controls, {
        action: "remove",
        replace: false,
        reason: "unused",
        source: "llm",
      });

      expect(controls.targetCall).not.toHaveBeenCalled();
      expect(controls.remove).not.toHaveBeenCalled();
      expect((yield* state()).observations).toEqual({});
    }),
  );

  it.effect("does not report an import successful until its files verify", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      send.mockClear();
      yield* TestClock.setTime(NOW);
      const item = stuckItem();
      const controls = mockClient([item]);
      controls.files = [importFile];
      controls.verifyImported.mockImplementation(() => Effect.succeed(false));
      yield* seedMature(item);

      const running = yield* Effect.forkChild(
        recovery(controls, {
          action: "import",
          reason: "safe import",
          source: "llm",
        }),
      );
      while (controls.importFiles.mock.calls.length === 0) yield* Effect.yieldNow;
      yield* Effect.yieldNow;
      yield* TestClock.adjust("2 seconds");
      yield* Fiber.join(running);

      const stored = yield* state();
      expect(controls.importFiles).toHaveBeenCalledTimes(1);
      expect(stored.actions[0]?.phase).toBe("uncertain");
      expect(send).toHaveBeenCalledWith(expect.stringContaining("Needs inspection"));
    }),
  );

  it.effect("removes a duplicate without requesting a replacement search", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const item = stuckItem();
      const controls = mockClient([item]);
      yield* seedMature(item);

      yield* recovery(controls, {
        action: "remove",
        replace: false,
        reason: "duplicate",
        source: "llm",
      });

      expect(controls.remove).toHaveBeenCalledTimes(1);
      expect(controls.search).not.toHaveBeenCalled();
      expect((yield* state()).actions[0]?.phase).toBe("done");
    }),
  );

  it.effect("never replays an import whose submission outcome is unknown", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const controls = mockClient([]);
      controls.verifyImported.mockImplementation(() => Effect.succeed(false));
      const action: RecoveryAction = {
        downloadId: "download-1",
        title: "Example.Movie.2026",
        target,
        files: [importFile],
        outputPath: "/downloads/example",
        decision: { action: "import", reason: "safe import", source: "llm" },
        phase: "uncertain",
        createdAt: NOW,
        updatedAt: NOW,
        error: "submission interrupted",
        notification: "sent",
      };
      yield* seedMature(stuckItem(), [action]);

      yield* recovery(controls, action.decision);

      expect(controls.importFiles).not.toHaveBeenCalled();
      expect(controls.command).not.toHaveBeenCalled();
      expect((yield* state()).actions[0]?.phase).toBe("uncertain");
    }),
  );

  it.effect("searches once after confirming a replacement removal", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const item = stuckItem();
      const controls = mockClient([item]);
      yield* seedMature(item);
      const decision: Decision = {
        action: "remove",
        replace: true,
        reason: "failed download",
        source: "llm",
      };

      yield* recovery(controls, decision);
      yield* recovery(controls, decision);

      expect(controls.remove).toHaveBeenCalledTimes(1);
      expect(controls.verifyRemoved).toHaveBeenCalled();
      expect(controls.search).toHaveBeenCalledTimes(1);
      expect(controls.search).toHaveBeenCalledWith(
        expect.objectContaining({ id: target.id }),
      );
    }),
  );

  it.effect("aborts when the queue changes during assessment", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const item = stuckItem();
      const controls = mockClient([item]);
      yield* seedMature(item);
      const changingAssessment: NonNullable<RecoveryOptions["assess"]> = vi.fn(() =>
        Effect.sync(() => {
          controls.queue = [
            stuckItem({
              statusMessages: [
                { title: "Import failed", messages: ["Failure changed"] },
              ],
            }),
          ];
          return {
            action: "remove" as const,
            replace: false,
            reason: "duplicate",
            source: "llm" as const,
          };
        }),
      );

      yield* runRecovery([controls.client], logger, {
        assess: changingAssessment,
        send,
      });

      expect(controls.remove).not.toHaveBeenCalled();
      expect(controls.importFiles).not.toHaveBeenCalled();
      expect((yield* state()).actions).toEqual([]);
    }),
  );

  it.effect("defers a replacement while its search backoff is active", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* TestClock.setTime(NOW);
      const item = stuckItem({ id: 2, downloadId: "download-2" });
      const controls = mockClient([item]);
      const prior: RecoveryAction = {
        downloadId: "download-1",
        title: "Earlier failed download",
        target,
        files: [],
        outputPath: "/downloads/earlier",
        decision: {
          action: "remove",
          replace: true,
          reason: "failed download",
          source: "rules",
        },
        phase: "done",
        createdAt: NOW - 1,
        updatedAt: NOW - 1,
        notification: "sent",
      };
      yield* seedMature(item, [prior]);

      yield* recovery(controls, {
        action: "remove",
        replace: true,
        reason: "another failed download",
        source: "llm",
      });

      expect(controls.remove).not.toHaveBeenCalled();
      expect(controls.search).not.toHaveBeenCalled();
      const stored = yield* state();
      expect(stored.actions).toHaveLength(1);
      expect(stored.observations["download-2"]?.reason).toBe(
        "Replacement search budget/backoff reached",
      );
    }),
  );

  it("allows another search after backoff but enforces the three-attempt budget", () => {
    const replacement = (createdAt: number, downloadId: string): RecoveryAction => ({
      downloadId,
      title: "Example.Movie.2026",
      target,
      files: [],
      outputPath: `/downloads/${downloadId}`,
      decision: {
        action: "remove",
        replace: true,
        reason: "failed download",
        source: "rules",
      },
      phase: "done",
      createdAt,
      updatedAt: createdAt,
      notification: "sent",
    });
    const sixHours = 6 * 60 * 60_000;

    expect(canReplace([replacement(NOW - 1, "one")], target, NOW)).toBe(false);
    expect(canReplace([replacement(NOW - sixHours, "one")], target, NOW)).toBe(true);
    expect(
      canReplace(
        [
          replacement(NOW - sixHours, "one"),
          replacement(NOW - sixHours, "two"),
          replacement(NOW - sixHours, "three"),
        ],
        target,
        NOW,
      ),
    ).toBe(false);
  });
});
