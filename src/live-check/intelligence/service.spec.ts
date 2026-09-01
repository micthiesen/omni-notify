import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { Deferred, Effect, Exit, Fiber } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Platform } from "../platforms/index.js";
import type { Streamer } from "../streamers.js";
import {
  DESTINY_CONFIRMED_EVENT_TITLE,
  LivestreamDiagnosticsEntity,
  LivestreamFeedbackEntity,
  LivestreamIntelligenceEntity,
  LivestreamIntelligenceEventEntity,
  recordLivestreamEvent,
  saveLivestreamIntelligence,
} from "./persistence.js";
import {
  isDestinyOwnedStream,
  LivestreamIntelligenceService,
  EffectWorkQueue,
  viewerCountForAnomaly,
} from "./service.js";

describe("EffectWorkQueue close", () => {
  it("closes admission before draining work already accepted", async () => {
    const queue = new EffectWorkQueue(1);
    const release = Effect.runSync(Deferred.make<void>());
    await Effect.runPromise(queue.fork(Deferred.await(release)));

    const closing = Effect.runFork(queue.close());
    await vi.waitFor(() => expect(queue.pending).toBe(1));
    const late = await Effect.runPromiseExit(queue.run(Effect.void));
    expect(Exit.isFailure(late)).toBe(true);
    expect(Effect.runSync(Fiber.poll(closing))._tag).toBe("None");

    await Effect.runPromise(Deferred.succeed(release, undefined));
    await Effect.runPromise(Fiber.join(closing));
    expect(queue.pending).toBe(0);
    expect(queue.size).toBe(0);
  });

  it("drains an admitted job interrupted while waiting for a permit", async () => {
    const queue = new EffectWorkQueue(1);
    const release = Effect.runSync(Deferred.make<void>());
    await Effect.runPromise(queue.fork(Deferred.await(release)));
    await vi.waitFor(() => expect(queue.pending).toBe(1));
    const waiting = Effect.runFork(queue.run(Effect.never));
    await vi.waitFor(() => expect(queue.size).toBe(1));
    const closing = Effect.runFork(queue.close());

    await Effect.runPromise(Fiber.interrupt(waiting));
    expect(queue.size).toBe(0);
    await Effect.runPromise(Deferred.succeed(release, undefined));
    await Effect.runPromise(Fiber.join(closing));
    expect(queue.pending).toBe(0);
  });

  it("cannot leak admission when fork is interrupted during startup", async () => {
    const queue = new EffectWorkQueue(1);
    const release = Effect.runSync(Deferred.make<void>());
    const forking = Effect.runFork(queue.fork(Deferred.await(release)));

    await Effect.runPromise(Fiber.interrupt(forking));
    await Effect.runPromise(Deferred.succeed(release, undefined));
    await Effect.runPromise(queue.close());

    expect(queue.pending).toBe(0);
    expect(queue.size).toBe(0);
  });
});

const { capture, detectDestiny } = vi.hoisted(() => ({
  capture: vi.fn(() =>
    Effect.succeed({
      samples: new Float32Array(18 * 16_000),
      sampleRate: 16_000,
      durationSeconds: 18,
    }),
  ),
  detectDestiny: vi.fn(() => ({
    confidence: 0.755,
    matchedWindows: 1,
    checkedWindows: 4,
  })),
}));

vi.mock("@micthiesen/mitools/pushover", () => ({ notify: vi.fn() }));
vi.mock("../../utils/config.js", () => ({
  default: {
    LIVESTREAM_MODEL_DIR: "/unused/models",
    LIVESTREAM_DESTINY_VOICEPRINT_PATH: "/unused/destiny.json",
    LIVESTREAM_DESTINY_SPEAKER_THRESHOLD: 0.62,
    LIVESTREAM_MAX_VOICE_TARGETS: 3,
    LIVESTREAM_VOICE_SAMPLE_SECONDS: 18,
    LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS: 45,
    LIVESTREAM_SUMMARY_SAMPLE_SECONDS: 75,
    LIVESTREAM_SUMMARY_INTERVAL_SECONDS: 480,
    LIVESTREAM_MONTHLY_BUDGET_USD: 3,
    PUSHOVER_LIVE_TOKEN: "fake-live-token",
  },
}));
vi.mock("./audio.js", () => ({
  LivestreamAudioCapture: class {
    public captureEffect = capture;
  },
}));
vi.mock("./localSpeech.js", () => ({
  LocalSpeechRuntime: class {
    public readonly hasVoiceprint = true;
    public detectDestinyEffect = vi.fn((_samples: Float32Array) =>
      Effect.sync(() => detectDestiny()),
    );
    public transcribeEffect = vi.fn(() => Effect.succeed("unused transcript"));
  },
}));
vi.mock("./classifier.js", () => ({
  isTranscriptAlertType: () => false,
  LivestreamClassifier: class {},
  livestreamSpendCents: () => 0,
}));

const speechDependency = {
  hasVoiceprint: true,
  detectDestinyEffect: vi.fn((_samples: Float32Array) =>
    Effect.sync(() => detectDestiny()),
  ),
  transcribeEffect: vi.fn(() => Effect.succeed("unused transcript")),
};

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.ERROR,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: `/tmp/omni-livestream-service-${process.pid}.db`,
  },
});

const sessionStartedAt = 10_000;
const streamer: Streamer = {
  id: "darius",
  displayName: "Darius",
  bindings: [{ platform: Platform.Kick, username: "dariusirl" }],
  tier: "background",
  dgg: { viewers: 599, hosted: true },
};
const observation = {
  streamer,
  status: {
    streamerId: streamer.id,
    isLive: true as const,
    primary: streamer.bindings[0]!,
    primaryTitle: "Live debate",
    startedAt: new Date(sessionStartedAt),
    maxViewerCount: 700,
    viewerCount: 700,
  },
  wentLive: false,
  titleChanged: false,
};

function candidateStreamer(overrides: Partial<Streamer>): Streamer {
  return {
    id: "dgg:youtube:video-id",
    displayName: "Guest Channel",
    bindings: [{ platform: Platform.YouTube, username: "video-id" }],
    tier: "background",
    dgg: { hosted: false, viewers: 100 },
    ...overrides,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(notify).mockReset();
  vi.mocked(notify).mockResolvedValue(undefined);
  capture.mockClear();
  detectDestiny.mockClear();
  LivestreamFeedbackEntity.deleteAll();
  LivestreamDiagnosticsEntity.deleteAll();
  LivestreamIntelligenceEventEntity.deleteAll();
  LivestreamIntelligenceEntity.deleteAll();
});

describe("isDestinyOwnedStream", () => {
  it("rejects the configured Destiny stream", () => {
    expect(isDestinyOwnedStream(candidateStreamer({ id: "destiny" }))).toBe(true);
  });

  it("rejects a DGG-discovered Destiny video with a dynamic ID", () => {
    expect(
      isDestinyOwnedStream(
        candidateStreamer({
          id: "dgg:youtube:abc123",
          displayName: "Destiny",
        }),
      ),
    ).toBe(true);
  });

  it("rejects canonical Destiny usernames regardless of display name", () => {
    expect(
      isDestinyOwnedStream(
        candidateStreamer({
          bindings: [{ platform: Platform.YouTube, username: "@Destiny" }],
        }),
      ),
    ).toBe(true);
  });

  it("keeps a third-party DGG stream eligible", () => {
    expect(isDestinyOwnedStream(candidateStreamer({}))).toBe(false);
  });
});

describe("viewerCountForAnomaly", () => {
  it("uses the sticky primary source instead of summing overlapping bindings", () => {
    expect(
      viewerCountForAnomaly({
        ...observation.status,
        viewerCount: 1_700,
        sources: [
          {
            platform: Platform.Kick,
            username: "dariusirl",
            title: "Live debate",
            viewerCount: 700,
          },
          {
            platform: Platform.YouTube,
            username: "darius",
            title: "Live debate",
            viewerCount: 1_000,
          },
        ],
      }),
    ).toBe(700);
  });

  it("falls back to the aggregate for records without source observations", () => {
    expect(viewerCountForAnomaly(observation.status)).toBe(700);
  });

  it("does not substitute secondary viewers when the primary count is missing", () => {
    expect(
      viewerCountForAnomaly({
        ...observation.status,
        viewerCount: 1_000,
        sources: [
          {
            platform: Platform.Kick,
            username: "dariusirl",
            title: "Live debate",
          },
          {
            platform: Platform.YouTube,
            username: "darius",
            title: "Live debate",
            viewerCount: 1_000,
          },
        ],
      }),
    ).toBeNull();
  });
});

describe("LivestreamIntelligenceService Destiny alert recovery", () => {
  it("retries a previously confirmed alert once and persists durable dedup", async () => {
    saveLivestreamIntelligence({
      streamerId: streamer.id,
      sessionStartedAt,
      relevanceScore: 20,
      relevanceReasons: [],
      chapters: [],
      destinyPresence: {
        state: "possible",
        confidence: 0.765,
        detectedAt: 20_000,
        reason: "Awaiting confirmation",
      },
      updatedAt: 20_000,
    });
    recordLivestreamEvent({
      streamerId: streamer.id,
      sessionStartedAt,
      kind: "voice",
      status: "success",
      title: DESTINY_CONFIRMED_EVENT_TITLE,
      detail: "Destiny is participating in the live conversation.",
      metrics: {
        speakerConfidence: 0.7059429831388251,
        assessmentConfidence: 0.91,
      },
      createdAt: 21_000,
    });

    const first = new LivestreamIntelligenceService(new Logger("VoiceRetryTest"), {
      speech: speechDependency,
    });
    await Effect.runPromise(first.observeLive(observation));
    await Effect.runPromise(first.afterTick());
    await Effect.runPromise(first.close());

    expect(notify).toHaveBeenCalledTimes(1);
    expect(vi.mocked(notify).mock.calls[0]?.[0]).toMatchObject({
      title: "Destiny is on Darius",
      token: "fake-live-token",
    });
    const delivered = LivestreamIntelligenceEntity.get({ streamerId: "darius" });
    expect(delivered).toMatchObject({
      destinyPresence: { state: "confirmed" },
      latestAlert: { type: "destiny_guest" },
      alertedAtByType: { destiny_guest: expect.any(Number) },
    });

    saveLivestreamIntelligence({
      ...delivered!,
      latestAlert: {
        alertId: "later-alert",
        type: "debate",
        title: "Debate",
        message: "A debate started",
        reason: "Transcript evidence",
        confidence: 0.9,
        createdAt: Date.now(),
      },
    });
    const second = new LivestreamIntelligenceService(new Logger("VoiceDedupTest"), {
      speech: speechDependency,
    });
    await Effect.runPromise(second.observeLive(observation));
    await Effect.runPromise(second.afterTick());
    await Effect.runPromise(second.close());

    expect(notify).toHaveBeenCalledTimes(1);
  });
});

describe("LivestreamIntelligenceService viewer surge alerts", () => {
  it("sends one durable alert for a sustained late primary-source surge", async () => {
    const primaryStreamer: Streamer = { ...streamer, tier: "primary" };
    const clockBase = 2_000_000_000_000;
    let now = clockBase;
    const nowSpy = vi.spyOn(Date, "now").mockImplementation(() => now);
    const makeObservation = (viewerCount: number) => ({
      ...observation,
      streamer: primaryStreamer,
      status: {
        ...observation.status,
        viewerCount: viewerCount + 1_000,
        sources: [
          {
            platform: Platform.Kick,
            username: "dariusirl",
            title: "Live debate",
            viewerCount,
          },
          {
            platform: Platform.YouTube,
            username: "darius",
            title: "Live debate",
            viewerCount: 1_000,
          },
        ],
      },
    });
    const observeAtMinute = async (
      service: LivestreamIntelligenceService,
      minute: number,
      viewers: number,
    ) => {
      now = clockBase + minute * 60_000;
      await Effect.runPromise(service.observeLive(makeObservation(viewers)));
    };

    const first = new LivestreamIntelligenceService(new Logger("SurgeTest"), {
      speech: speechDependency,
    });
    for (let minute = 0; minute < 15; minute += 1) {
      await observeAtMinute(first, minute, 200);
    }
    await observeAtMinute(first, 16, 430);
    expect(notify).not.toHaveBeenCalled();
    await observeAtMinute(first, 17, 440);
    expect(LivestreamIntelligenceEntity.get({ streamerId: "darius" })).toMatchObject({
      relevanceScore: 79,
      trend: {
        anomalous: true,
        baselineViewers: 200,
        candidateObservations: 2,
      },
    });
    await vi.waitFor(() => expect(notify).toHaveBeenCalledTimes(1));
    expect(vi.mocked(notify).mock.calls[0]?.[0]).toMatchObject({
      title: "Darius is surging",
      message: expect.stringContaining("440 vs 200 baseline"),
    });
    expect(LivestreamIntelligenceEntity.get({ streamerId: "darius" })).toMatchObject({
      alertedAtByType: { viewer_surge: expect.any(Number) },
    });

    const restarted = new LivestreamIntelligenceService(
      new Logger("SurgeRestartTest"),
      { speech: speechDependency },
    );
    for (let minute = 40; minute < 55; minute += 1) {
      await observeAtMinute(restarted, minute, 200);
    }
    await observeAtMinute(restarted, 55, 430);
    await observeAtMinute(restarted, 56, 440);
    await vi.waitFor(() => expect(notify).toHaveBeenCalledTimes(1));
    nowSpy.mockRestore();
  });
});
