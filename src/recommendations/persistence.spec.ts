import { describe, expect, it } from "vitest";
import {
  computeExcludedCanonicalIds,
  formatFeedbackDigestFrom,
  ON_DECK_LIMIT,
  type RecommendationData,
  RecommendationStatus,
  selectOnDeck,
} from "./persistence.js";
import { MediaType } from "./types.js";

const NOW = 1_800_000_000_000;

function rec(
  recommendationId: string,
  canonicalId: string,
  overrides: Partial<RecommendationData> = {},
): RecommendationData {
  return {
    recommendationId,
    canonicalId,
    tmdbId: Number(canonicalId.split(":")[2]),
    mediaType: MediaType.Movie,
    title: recommendationId,
    status: RecommendationStatus.Ignored,
    runDate: "2026-01-01",
    recommendedAt: NOW - 200 * 24 * 60 * 60 * 1000,
    ...overrides,
  };
}

describe("recommendation persistence rules", () => {
  it("preserves multiple attempts for the same canonical title in calculations", () => {
    const records = [
      rec("first", "tmdb:movie:1"),
      rec("second", "tmdb:movie:1", { recommendedAt: NOW - 1000 }),
    ];
    expect(records.map((item) => item.recommendationId)).toEqual(["first", "second"]);
    expect(computeExcludedCanonicalIds(records, NOW)).toContain("tmdb:movie:1");
  });

  it("lets the latest explicit feedback correct an older negative response", () => {
    const records = [
      rec("old", "tmdb:movie:2", {
        feedback: "not_for_me",
        feedbackAt: NOW - 10_000,
      }),
      rec("new", "tmdb:movie:2", {
        feedback: "good_pick",
        feedbackAt: NOW - 5_000,
      }),
    ];
    expect(computeExcludedCanonicalIds(records, NOW)).not.toContain("tmdb:movie:2");
  });

  it("uses only good and not-for-me feedback as taste evidence", () => {
    const digest = formatFeedbackDigestFrom([
      rec("Loved It", "tmdb:movie:3", { feedback: "good_pick" }),
      rec("No Thanks", "tmdb:movie:4", { feedback: "not_for_me" }),
      rec("Seen It", "tmdb:movie:5", { feedback: "already_watched" }),
    ]);
    expect(digest).toContain("Good picks: Loved It");
    expect(digest).toContain("Not for me: No Thanks");
    expect(digest).not.toContain("Seen It");
  });

  it("uses a short retry backoff for failed acquisition attempts", () => {
    const recentFailure = rec("recent", "tmdb:movie:6", {
      status: RecommendationStatus.Failed,
      recommendedAt: NOW - 12 * 60 * 60 * 1000,
    });
    const oldFailure = rec("old", "tmdb:movie:7", {
      status: RecommendationStatus.Failed,
      recommendedAt: NOW - 2 * 24 * 60 * 60 * 1000,
    });
    const excluded = computeExcludedCanonicalIds([recentFailure, oldFailure], NOW);
    expect(excluded).toContain("tmdb:movie:6");
    expect(excluded).not.toContain("tmdb:movie:7");
  });
});

describe("selectOnDeck", () => {
  const notified = (id: string, ageMs: number): RecommendationData =>
    rec(id, `tmdb:movie:${id}`, {
      status: RecommendationStatus.Notified,
      recommendedAt: NOW - ageMs,
    });

  it("excludes pending rows (not yet delivered)", () => {
    const picks = selectOnDeck([
      notified("a", 1000),
      rec("p", "tmdb:movie:9", {
        status: RecommendationStatus.Pending,
        recommendedAt: NOW,
      }),
    ]);
    expect(picks.map((r) => r.recommendationId)).toEqual(["a"]);
  });

  it("returns the newest first and caps at the limit", () => {
    const picks = selectOnDeck([
      notified("oldest", 5000),
      notified("newest", 1000),
      notified("mid", 3000),
      notified("older", 4000),
      notified("newer", 2000),
    ]);
    expect(picks).toHaveLength(ON_DECK_LIMIT);
    expect(picks.map((r) => r.recommendationId)).toEqual([
      "newest",
      "newer",
      "mid",
      "older",
    ]);
  });
});
