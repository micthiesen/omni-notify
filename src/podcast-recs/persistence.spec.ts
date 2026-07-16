import { describe, expect, it } from "vitest";
import {
  computePodcastExclusions,
  formatPodcastFeedbackDigestFrom,
  type PodcastRecommendationData,
  PodcastRecommendationStatus,
} from "./persistence.js";

const NOW = Date.UTC(2026, 6, 16);
const DAY = 24 * 60 * 60 * 1000;

function rec(
  overrides: Partial<PodcastRecommendationData> = {},
): PodcastRecommendationData {
  return {
    recommendationId: "r1",
    episodeId: "itunes:1#guid-1",
    showId: "itunes:1",
    showTitle: "The Gray Area",
    episodeTitle: "What is consciousness?",
    feedUrl: "https://feeds.example.com/grayarea",
    episodeGuid: "guid-1",
    publishedAt: NOW - 3 * DAY,
    status: PodcastRecommendationStatus.Notified,
    runDate: "2026-07-13",
    recommendedAt: NOW - 3 * DAY,
    ...overrides,
  };
}

describe("computePodcastExclusions", () => {
  it("excludes delivered episodes permanently and their show during cooldown", () => {
    const { episodeIds, showIds } = computePodcastExclusions([rec()], NOW);
    expect(episodeIds.has("itunes:1#guid-1")).toBe(true);
    expect(showIds.has("itunes:1")).toBe(true);
  });

  it("keeps the episode excluded after the show cooldown lapses", () => {
    const old = rec({ recommendedAt: NOW - 45 * DAY });
    const { episodeIds, showIds } = computePodcastExclusions([old], NOW);
    expect(episodeIds.has("itunes:1#guid-1")).toBe(true);
    expect(showIds.has("itunes:1")).toBe(false);
  });

  it("gives failed rows only a short retry exclusion", () => {
    const failedRecent = rec({
      status: PodcastRecommendationStatus.Failed,
      recommendedAt: NOW - DAY / 2,
    });
    expect(computePodcastExclusions([failedRecent], NOW).episodeIds.size).toBe(1);

    const failedOld = rec({
      status: PodcastRecommendationStatus.Failed,
      recommendedAt: NOW - 2 * DAY,
    });
    expect(computePodcastExclusions([failedOld], NOW).episodeIds.size).toBe(0);
  });

  it("excludes not-for-me shows permanently", () => {
    const old = rec({
      recommendedAt: NOW - 200 * DAY,
      feedback: "not_for_me",
      feedbackAt: NOW - 199 * DAY,
    });
    expect(computePodcastExclusions([old], NOW).showIds.has("itunes:1")).toBe(true);
  });

  it("lets newer feedback correct an earlier not-for-me", () => {
    const bad = rec({
      recommendationId: "r1",
      episodeId: "itunes:1#guid-1",
      recommendedAt: NOW - 200 * DAY,
      feedback: "not_for_me",
      feedbackAt: NOW - 199 * DAY,
    });
    const good = rec({
      recommendationId: "r2",
      episodeId: "itunes:1#guid-2",
      episodeGuid: "guid-2",
      recommendedAt: NOW - 100 * DAY,
      feedback: "good_pick",
      feedbackAt: NOW - 99 * DAY,
    });
    expect(computePodcastExclusions([bad, good], NOW).showIds.has("itunes:1")).toBe(
      false,
    );
  });
});

describe("formatPodcastFeedbackDigestFrom", () => {
  it("reports latest feedback per show, grouped by polarity", () => {
    const digest = formatPodcastFeedbackDigestFrom([
      rec({ feedback: "good_pick", feedbackAt: NOW - DAY }),
      rec({
        recommendationId: "r2",
        showId: "itunes:2",
        showTitle: "Some Grifty Show",
        episodeId: "itunes:2#g",
        feedback: "not_for_me",
        feedbackAt: NOW - 2 * DAY,
      }),
    ]);
    expect(digest).toContain("Good picks: The Gray Area");
    expect(digest).toContain("Not for me: Some Grifty Show");
  });

  it("handles the empty case", () => {
    expect(formatPodcastFeedbackDigestFrom([])).toBe(
      "No explicit podcast feedback yet.",
    );
  });
});
