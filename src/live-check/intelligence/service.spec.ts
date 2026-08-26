import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
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
import { isDestinyOwnedStream, LivestreamIntelligenceService } from "./service.js";

const { capture, detectDestiny } = vi.hoisted(() => ({
  capture: vi.fn(async () => ({
    samples: new Float32Array(18 * 16_000),
    sampleRate: 16_000,
    durationSeconds: 18,
  })),
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
    public capture = capture;
  },
}));
vi.mock("./localSpeech.js", () => ({
  LocalSpeechRuntime: class {
    public readonly hasVoiceprint = true;
    public detectDestiny = detectDestiny;
    public transcribe = vi.fn(async () => "unused transcript");
  },
}));
vi.mock("./classifier.js", () => ({
  isTranscriptAlertType: () => false,
  LivestreamClassifier: class {},
  livestreamSpendCents: () => 0,
}));

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
  vi.mocked(notify).mockReset();
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

    const first = new LivestreamIntelligenceService(new Logger("VoiceRetryTest"));
    first.observeLive(observation);
    first.afterTick();
    await first.close();

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
    const second = new LivestreamIntelligenceService(new Logger("VoiceDedupTest"));
    second.observeLive(observation);
    second.afterTick();
    await second.close();

    expect(notify).toHaveBeenCalledTimes(1);
  });
});
