import { describe, expect, it } from "vitest";
import {
  computeExcludedCanonicalIds,
  formatFeedbackDigestFrom,
  type RecommendationData,
  RecommendationStatus,
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
