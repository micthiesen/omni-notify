import { describe, expect, it } from "vitest";
import { it as effectIt } from "@effect/vitest";
import { Effect } from "effect";
import type { PodcastWriteResult } from "./account.js";
import type { PodcastRecommendationData } from "./persistence.js";
import { PodcastRecommendationStatus } from "./persistence.js";
import {
  enqueueAndPersistEffect,
  listenHistorySince,
  loadPipelineTasteSeedEffect,
  PodcastPipelineError,
  toQueueResult,
} from "./pipeline.js";
import { TasteSeedError } from "./taste.js";

describe("loadPipelineTasteSeedEffect", () => {
  effectIt.effect("maps a malformed seed to a recoverable pipeline failure", () =>
    loadPipelineTasteSeedEffect("taste.md", () =>
      Effect.runPromise(Effect.succeed(" \n ")),
    ).pipe(
      Effect.flip,
      Effect.tap((error) =>
        Effect.sync(() => {
          expect(error).toBeInstanceOf(PodcastPipelineError);
          expect(error.cause).toBeInstanceOf(TasteSeedError);
          expect((error.cause as TasteSeedError).reason).toBe("malformed");
        }),
      ),
      Effect.asVoid,
    ),
  );
});

describe("enqueueAndPersistEffect", () => {
  effectIt.effect("persists the Castro result before returning to notification", () => {
    const events: string[] = [];
    return enqueueAndPersistEffect(
      () =>
        Effect.sync(() => {
          events.push("enqueue");
          return "added" as const;
        }),
      (result) => events.push(`persist:${result}`),
    ).pipe(
      Effect.tap(() => Effect.sync(() => events.push("notify-boundary"))),
      Effect.tap(() =>
        Effect.sync(() => {
          expect(events).toEqual(["enqueue", "persist:queued", "notify-boundary"]);
        }),
      ),
    );
  });
});

describe("toQueueResult", () => {
  const cases: [PodcastWriteResult, string][] = [
    ["added", "queued"],
    ["already_exists", "already_queued"],
    ["not_found", "not_queued"],
    ["unavailable", "not_queued"],
    ["error", "not_queued"],
    ["removed", "not_queued"],
  ];

  for (const [input, expected] of cases) {
    it(`maps ${input} → ${expected}`, () => {
      expect(toQueueResult(input)).toBe(expected);
    });
  }
});

describe("listenHistorySince", () => {
  const NOW = Date.UTC(2026, 6, 16);
  const DAY = 24 * 60 * 60 * 1000;
  const BUFFER = DAY;

  function rec(
    overrides: Partial<PodcastRecommendationData>,
  ): PodcastRecommendationData {
    return {
      recommendationId: "r",
      episodeId: "itunes:1#g",
      showId: "itunes:1",
      showTitle: "Show",
      episodeTitle: "Ep",
      feedUrl: "https://feeds.example.com/x",
      episodeGuid: "g",
      publishedAt: NOW,
      status: PodcastRecommendationStatus.Notified,
      runDate: "2026-07-16",
      recommendedAt: NOW,
      ...overrides,
    };
  }

  it("looks back to just before the oldest open delivery", () => {
    const open = [
      rec({ notifiedAt: NOW - 10 * DAY }),
      rec({ notifiedAt: NOW - 3 * DAY }),
    ];
    expect(listenHistorySince(open, NOW)).toBe(NOW - 10 * DAY - BUFFER);
  });

  it("covers a lingering open row fully rather than capping short of it", () => {
    // Regression: a cutoff that post-dates the delivery would hide the real
    // playback and mislabel a listened episode as ignored.
    const open = [rec({ notifiedAt: NOW - 200 * DAY })];
    expect(listenHistorySince(open, NOW)).toBe(NOW - 200 * DAY - BUFFER);
  });

  it("uses recommendedAt when never notified", () => {
    const open = [rec({ notifiedAt: undefined, recommendedAt: NOW - 5 * DAY })];
    expect(listenHistorySince(open, NOW)).toBe(NOW - 5 * DAY - BUFFER);
  });

  it("returns now when nothing is open (nothing to cover)", () => {
    expect(listenHistorySince([], NOW)).toBe(NOW);
  });
});
