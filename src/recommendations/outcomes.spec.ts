import { describe, expect, it } from "vitest";
import { decideOutcomes, IGNORE_WINDOW_MS, type OutcomeInputs } from "./outcomes.js";
import { type RecommendationData, RecommendationStatus } from "./persistence.js";
import { MediaType } from "./types.js";

const NOW = 1_750_000_000_000;

function makeRec(overrides: Partial<RecommendationData> = {}): RecommendationData {
  return {
    canonicalId: "tmdb:movie:123",
    tmdbId: 123,
    mediaType: MediaType.Movie,
    title: "Test Movie",
    status: RecommendationStatus.Notified,
    runDate: "2026-07-01",
    recommendedAt: NOW - 5 * 24 * 60 * 60 * 1000,
    notifiedAt: NOW - 5 * 24 * 60 * 60 * 1000,
    ...overrides,
  };
}

function makeInputs(overrides: Partial<OutcomeInputs> = {}): OutcomeInputs {
  return {
    watched: new Map(),
    inProgress: new Map(),
    watchlistIds: new Set(),
    inProgressAvailable: true,
    watchlistComplete: true,
    now: NOW,
    ...overrides,
  };
}

describe("decideOutcomes", () => {
  it("labels watched when completion is at or above the threshold", () => {
    const changes = decideOutcomes(
      [makeRec()],
      makeInputs({
        watched: new Map([["tmdb:movie:123", { completion: 0.92, viewCount: 1 }]]),
      }),
    );
    expect(changes).toHaveLength(1);
    expect(changes[0].status).toBe(RecommendationStatus.Watched);
  });

  it("labels watched on a view when the backend reports no completion", () => {
    const changes = decideOutcomes(
      [makeRec()],
      makeInputs({
        watched: new Map([["tmdb:movie:123", { viewCount: 1 }]]),
      }),
    );
    expect(changes[0]?.status).toBe(RecommendationStatus.Watched);
  });

  it("does not label watched below the completion threshold", () => {
    const changes = decideOutcomes(
      [makeRec()],
      makeInputs({
        watched: new Map([["tmdb:movie:123", { completion: 0.3, viewCount: 1 }]]),
        inProgress: new Map([["tmdb:movie:123", { progress: 0.3 }]]),
      }),
    );
    expect(changes).toHaveLength(0);
  });

  it("labels abandoned when started but stalled and no longer in progress", () => {
    const changes = decideOutcomes(
      [makeRec()],
      makeInputs({
        watched: new Map([["tmdb:movie:123", { completion: 0.25, viewCount: 1 }]]),
      }),
    );
    expect(changes[0]?.status).toBe(RecommendationStatus.Abandoned);
  });

  it("labels abandoned when removed from the watchlist unwatched", () => {
    const changes = decideOutcomes(
      [makeRec({ watchlistResult: "added" })],
      makeInputs(),
    );
    // Not on the watchlist anymore, never watched.
    expect(changes[0]?.status).toBe(RecommendationStatus.Abandoned);
  });

  it("does not label abandoned for skipped watchlist writes missing from the watchlist", () => {
    const changes = decideOutcomes(
      [makeRec({ watchlistResult: "skipped" })],
      makeInputs(),
    );
    expect(changes).toHaveLength(0);
  });

  it("labels ignored after the ignore window", () => {
    const changes = decideOutcomes(
      [makeRec({ notifiedAt: NOW - IGNORE_WINDOW_MS - 1000 })],
      makeInputs(),
    );
    expect(changes[0]?.status).toBe(RecommendationStatus.Ignored);
  });

  it("leaves in-progress items open past the ignore window", () => {
    const changes = decideOutcomes(
      [makeRec({ notifiedAt: NOW - IGNORE_WINDOW_MS - 1000 })],
      makeInputs({
        inProgress: new Map([["tmdb:movie:123", { progress: 0.4 }]]),
      }),
    );
    expect(changes).toHaveLength(0);
  });

  it("suppresses abandoned when the in-progress source is unavailable", () => {
    const changes = decideOutcomes(
      [makeRec()],
      makeInputs({
        watched: new Map([["tmdb:movie:123", { completion: 0.25, viewCount: 1 }]]),
        inProgressAvailable: false,
      }),
    );
    expect(changes).toHaveLength(0);
  });

  it("suppresses ignored when the in-progress source is unavailable", () => {
    const changes = decideOutcomes(
      [makeRec({ notifiedAt: NOW - IGNORE_WINDOW_MS - 1000 })],
      makeInputs({ inProgressAvailable: false }),
    );
    expect(changes).toHaveLength(0);
  });

  it("suppresses watchlist-removal abandonment when the watchlist is incomplete", () => {
    const changes = decideOutcomes(
      [makeRec({ watchlistResult: "added" })],
      makeInputs({ watchlistComplete: false }),
    );
    expect(changes).toHaveLength(0);
  });

  it("still labels watched when absence-based inputs are unavailable", () => {
    const changes = decideOutcomes(
      [makeRec()],
      makeInputs({
        watched: new Map([["tmdb:movie:123", { completion: 0.95, viewCount: 1 }]]),
        inProgressAvailable: false,
        watchlistComplete: false,
      }),
    );
    expect(changes[0]?.status).toBe(RecommendationStatus.Watched);
  });

  it("ignores rows that are not in notified status", () => {
    const changes = decideOutcomes(
      [
        makeRec({
          status: RecommendationStatus.Pending,
          notifiedAt: NOW - IGNORE_WINDOW_MS - 1000,
        }),
      ],
      makeInputs(),
    );
    expect(changes).toHaveLength(0);
  });
});
