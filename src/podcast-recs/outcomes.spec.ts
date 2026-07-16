import { describe, expect, it } from "vitest";
import type { ListenedEpisode } from "./account.js";
import {
  ABANDONED_INACTIVITY_MS,
  decideEpisodeOutcomes,
  IGNORE_WINDOW_MS,
} from "./outcomes.js";
import {
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
    publishedAt: NOW - 10 * DAY,
    status: PodcastRecommendationStatus.Notified,
    runDate: "2026-07-06",
    recommendedAt: NOW - 10 * DAY,
    notifiedAt: NOW - 10 * DAY,
    ...overrides,
  };
}

function listened(overrides: Partial<ListenedEpisode> = {}): ListenedEpisode {
  return {
    showTitle: "The Gray Area",
    episodeTitle: "What is consciousness?",
    episodeGuid: "guid-1",
    listenedAt: NOW - DAY,
    ...overrides,
  };
}

describe("decideEpisodeOutcomes", () => {
  it("labels listened at/above the completion threshold", () => {
    const changes = decideEpisodeOutcomes(
      [rec()],
      [listened({ completion: 0.95 })],
      NOW,
    );
    expect(changes).toHaveLength(1);
    expect(changes[0].status).toBe(PodcastRecommendationStatus.Listened);
  });

  it("counts a playback event without completion data as listened", () => {
    const changes = decideEpisodeOutcomes([rec()], [listened()], NOW);
    expect(changes[0]?.status).toBe(PodcastRecommendationStatus.Listened);
  });

  it("matches by normalized titles when the guid differs", () => {
    const changes = decideEpisodeOutcomes(
      [rec()],
      [
        listened({
          episodeGuid: "castro-internal-id",
          showTitle: "the gray area",
          episodeTitle: "What Is Consciousness?!",
          completion: 0.9,
        }),
      ],
      NOW,
    );
    expect(changes[0]?.status).toBe(PodcastRecommendationStatus.Listened);
  });

  it("labels abandoned after stalling below the threshold", () => {
    const changes = decideEpisodeOutcomes(
      [rec({ recommendedAt: NOW - 20 * DAY, notifiedAt: NOW - 20 * DAY })],
      [
        listened({
          completion: 0.3,
          listenedAt: NOW - ABANDONED_INACTIVITY_MS - DAY,
        }),
      ],
      NOW,
    );
    expect(changes[0]?.status).toBe(PodcastRecommendationStatus.Abandoned);
  });

  it("leaves a recently-started episode open", () => {
    const changes = decideEpisodeOutcomes(
      [rec()],
      [listened({ completion: 0.3, listenedAt: NOW - DAY })],
      NOW,
    );
    expect(changes).toHaveLength(0);
  });

  it("ignores playback that predates delivery", () => {
    const changes = decideEpisodeOutcomes(
      [rec({ notifiedAt: NOW - DAY })],
      [listened({ completion: 1, listenedAt: NOW - 2 * DAY })],
      NOW,
    );
    expect(changes).toHaveLength(0);
  });

  it("labels ignored after the window with no engagement", () => {
    const old = rec({
      recommendedAt: NOW - IGNORE_WINDOW_MS - DAY,
      notifiedAt: NOW - IGNORE_WINDOW_MS - DAY,
    });
    const changes = decideEpisodeOutcomes([old], [], NOW);
    expect(changes[0]?.status).toBe(PodcastRecommendationStatus.Ignored);
  });

  it("only labels notified rows", () => {
    const pending = rec({ status: PodcastRecommendationStatus.Pending });
    const changes = decideEpisodeOutcomes(
      [pending],
      [listened({ completion: 1 })],
      NOW,
    );
    expect(changes).toHaveLength(0);
  });
});
