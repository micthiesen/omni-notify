import { beforeEach, describe, expect, it, vi } from "vitest";
import { CandidateSource, MediaType } from "./types.js";

const mocks = vi.hoisted(() => ({
  fetchWatchHistory: vi.fn(),
  fetchInProgress: vi.fn(),
  fetchLibraryIndex: vi.fn(),
  fetchWatchlist: vi.fn(),
  addToWatchlist: vi.fn(),
  resolveIdentity: vi.fn(),
  fetchCandidateBuckets: vi.fn(),
  assemblePool: vi.fn(),
  enrichCandidates: vi.fn(),
  filterEligible: vi.fn(),
  shortlistCandidates: vi.fn(),
  researchFinalists: vi.fn(),
  selectRecommendation: vi.fn(),
  notify: vi.fn(),
  upsert: vi.fn(),
  patch: vi.fn(),
  getOpenRecommendations: vi.fn(),
  fetchTitleGenreIds: vi.fn(),
}));

vi.mock("@micthiesen/mitools/pushover", () => ({ notify: mocks.notify }));
vi.mock("../utils/config.js", () => ({
  default: {
    PUSHOVER_RECS_TOKEN: "push-token",
    RECS_PUBLIC_URL: "http://omni.test",
  },
}));
vi.mock("./mediaLibrary.js", () => ({
  fetchWatchHistory: mocks.fetchWatchHistory,
  fetchInProgress: mocks.fetchInProgress,
  fetchLibraryIndex: mocks.fetchLibraryIndex,
}));
vi.mock("./watchlist.js", () => ({
  fetchWatchlist: mocks.fetchWatchlist,
  addToWatchlist: mocks.addToWatchlist,
}));
vi.mock("./identity.js", () => ({
  RESOLUTION_CONFIDENCE_THRESHOLD: 0.8,
  resolveIdentity: mocks.resolveIdentity,
}));
vi.mock("./candidates.js", () => ({
  fetchCandidateBuckets: mocks.fetchCandidateBuckets,
  assemblePool: mocks.assemblePool,
  enrichCandidates: mocks.enrichCandidates,
}));
vi.mock("./filters.js", () => ({ filterEligible: mocks.filterEligible }));
vi.mock("./history.js", () => ({
  completedWatches: (items: unknown[]) => items,
  formatHistoryDigest: () => "history",
}));
vi.mock("./outcomes.js", () => ({ decideOutcomes: () => [] }));
vi.mock("./persistence.js", () => ({
  RecommendationStatus: {
    Pending: "pending",
    Notified: "notified",
    Failed: "failed",
  },
  RecommendationEntity: {
    getAll: () => [],
    upsert: mocks.upsert,
    patch: mocks.patch,
  },
  formatFeedbackDigest: () => "feedback",
  getExcludedCanonicalIds: () => new Set(),
  getOpenRecommendations: mocks.getOpenRecommendations,
}));
vi.mock("./shortlist.js", () => ({
  FINALIST_COUNT: 5,
  shortlistCandidates: mocks.shortlistCandidates,
}));
vi.mock("./selection.js", () => ({
  researchFinalists: mocks.researchFinalists,
  selectRecommendation: mocks.selectRecommendation,
}));
vi.mock("./tmdb/client.js", () => ({
  fetchTitleGenreIds: mocks.fetchTitleGenreIds,
}));
vi.mock("./taste/index.js", () => ({
  formatTasteProfileDigest: () => "taste profile",
}));

import { runRecommendationPipeline } from "./pipeline.js";

const watched = {
  guid: "watched",
  title: "Watched",
  mediaType: MediaType.Movie,
  externalIds: { tmdb: 1 },
  viewedAt: 100,
  viewCount: 1,
  completion: 1,
};
const library = {
  guid: "library",
  title: "Library",
  mediaType: MediaType.Movie,
  externalIds: { tmdb: 2 },
};
const tracked = {
  guid: "tracked",
  title: "Tracked",
  mediaType: MediaType.Movie,
  externalIds: { tmdb: 3 },
};
const candidate = {
  canonicalId: "tmdb:movie:4" as const,
  tmdbId: 4,
  mediaType: MediaType.Movie,
  title: "Candidate",
  overview: "",
  genreIds: [],
  voteAverage: 8,
  voteCount: 1000,
  popularity: 10,
  source: CandidateSource.Trending,
};
const enriched = { ...candidate, genres: [], inLibrary: false };
const scored = {
  candidate: enriched,
  tasteMatch: 80,
  novelty: 60,
  effortFit: 90,
  confidence: 0.9,
  risks: [],
  composite: 75,
};

function selection(candidateId: string) {
  return {
    decision: "select" as const,
    selected: {
      candidate_id: candidateId,
      why_for_user: "A fit",
      caveats: [],
      confidence: 0.9,
      notification: { title: "Candidate", message: "A fit" },
    },
    backup: null,
    no_add_reason: null,
  };
}

const logger = {
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
} as never;

describe("recommendation pipeline orchestration", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.fetchWatchHistory.mockResolvedValue({ status: "ok", value: [watched] });
    mocks.fetchInProgress.mockResolvedValue({ status: "ok", value: [] });
    mocks.fetchLibraryIndex.mockResolvedValue({ status: "ok", value: [library] });
    mocks.fetchWatchlist.mockResolvedValue({ status: "ok", value: [tracked] });
    mocks.resolveIdentity.mockImplementation(async (item: typeof watched) => ({
      canonicalId: `tmdb:${item.mediaType}:${item.externalIds.tmdb}`,
      confidence: 1,
      resolutionPath: "external-id",
    }));
    mocks.fetchTitleGenreIds.mockResolvedValue([]);
    mocks.getOpenRecommendations.mockReturnValue([]);
    mocks.fetchCandidateBuckets.mockResolvedValue([]);
    mocks.assemblePool.mockReturnValue([candidate]);
    mocks.filterEligible.mockReturnValue({ kept: [candidate], dropped: [] });
    mocks.enrichCandidates.mockResolvedValue([enriched]);
    mocks.shortlistCandidates.mockResolvedValue([scored]);
    mocks.researchFinalists.mockResolvedValue(new Map());
    mocks.selectRecommendation.mockResolvedValue({
      decision: "no_add",
      selected: null,
      backup: null,
      no_add_reason: "not today",
    });
  });

  it("keeps Plex availability and Arr tracked state in their correct roles", async () => {
    await runRecommendationPipeline(logger);

    const filterContext = mocks.filterEligible.mock.calls[0][1];
    expect(filterContext.watchlistIds).toEqual(new Set(["tmdb:movie:3"]));
    expect(mocks.enrichCandidates.mock.calls[0][1]).toEqual(new Set(["tmdb:movie:2"]));
  });

  it("fails closed and does not notify when acquisition fails", async () => {
    mocks.selectRecommendation.mockResolvedValue({
      decision: "select",
      selected: {
        candidate_id: candidate.canonicalId,
        why_for_user: "A fit",
        caveats: [],
        confidence: 0.9,
        notification: { title: "Candidate", message: "A fit" },
      },
      backup: null,
      no_add_reason: null,
    });
    mocks.addToWatchlist.mockResolvedValue({ result: "error" });

    await expect(runRecommendationPipeline(logger)).rejects.toThrow(
      "acquisition or notification failed",
    );
    expect(mocks.upsert).toHaveBeenCalledWith(
      expect.objectContaining({
        source: CandidateSource.Trending,
        genres: [],
        shortlistScores: {
          tasteMatch: 80,
          novelty: 60,
          effortFit: 90,
          composite: 75,
          risks: [],
        },
      }),
    );
    expect(mocks.notify).not.toHaveBeenCalled();
    expect(mocks.patch).toHaveBeenCalledWith(
      expect.objectContaining({ recommendationId: expect.any(String) }),
      expect.objectContaining({ status: "failed", watchlistResult: "error" }),
    );
  });

  it("supports a full dry run without acquisition or notification", async () => {
    mocks.selectRecommendation.mockResolvedValue({
      decision: "select",
      selected: {
        candidate_id: candidate.canonicalId,
        why_for_user: "A fit",
        caveats: [],
        confidence: 0.9,
        notification: { title: "Candidate", message: "A fit" },
      },
      backup: null,
      no_add_reason: null,
    });

    await expect(
      runRecommendationPipeline(logger, undefined, { dryRun: true }),
    ).resolves.toBe("dry_run: would recommend Candidate");
    expect(mocks.addToWatchlist).not.toHaveBeenCalled();
    expect(mocks.notify).not.toHaveBeenCalled();
  });

  it("researches once and repeatedly selects from the shrinking shortlist", async () => {
    const finalists = [4, 5, 6].map((tmdbId) => ({
      ...scored,
      candidate: {
        ...enriched,
        canonicalId: `tmdb:movie:${tmdbId}` as const,
        tmdbId,
        title: `Candidate ${tmdbId}`,
      },
    }));
    mocks.shortlistCandidates.mockResolvedValue(finalists);
    mocks.addToWatchlist.mockResolvedValue({ result: "added" });
    mocks.selectRecommendation
      .mockResolvedValueOnce(selection("tmdb:movie:4"))
      .mockResolvedValueOnce(selection("tmdb:movie:5"))
      .mockResolvedValueOnce({
        decision: "no_add",
        selected: null,
        backup: null,
        no_add_reason: "remaining fit is weak",
      });

    await expect(
      runRecommendationPipeline(logger, undefined, { maxRecommendations: 3 }),
    ).resolves.toBe(
      "recommended 2/3: Candidate 4, Candidate 5; stopped: no_add: remaining fit is weak",
    );

    expect(mocks.shortlistCandidates).toHaveBeenCalledWith(
      [enriched],
      expect.any(String),
      logger,
      undefined,
      6,
    );
    expect(mocks.researchFinalists).toHaveBeenCalledTimes(1);
    expect(mocks.selectRecommendation.mock.calls.map((call) => call[0])).toEqual([
      finalists,
      finalists.slice(1),
      finalists.slice(2),
    ]);
    expect(mocks.addToWatchlist).toHaveBeenCalledTimes(2);
    expect(mocks.notify).toHaveBeenCalledTimes(2);
  });

  it("rejects batch limits outside 1 through 10", async () => {
    await expect(
      runRecommendationPipeline(logger, undefined, { maxRecommendations: 11 }),
    ).rejects.toThrow("maxRecommendations must be an integer from 1 to 10");
    expect(mocks.fetchWatchHistory).not.toHaveBeenCalled();
  });

  it("records the first passive playback signal", async () => {
    const open = {
      recommendationId: "rec-1",
      canonicalId: "tmdb:movie:4",
      tmdbId: 4,
      mediaType: MediaType.Movie,
      title: "Candidate",
      status: "notified",
      runDate: "2026-07-15",
      recommendedAt: 1,
    };
    mocks.getOpenRecommendations.mockReturnValueOnce([open]);
    mocks.fetchInProgress.mockResolvedValue({
      status: "ok",
      value: [
        {
          guid: "candidate-progress",
          title: "Candidate",
          mediaType: MediaType.Movie,
          externalIds: { tmdb: 4 },
          progress: 0.2,
          lastViewedAt: 200,
        },
      ],
    });

    await runRecommendationPipeline(logger);

    expect(mocks.patch).toHaveBeenCalledWith(
      { recommendationId: "rec-1" },
      { startedAt: expect.any(Number) },
    );
  });

  it.each([
    ["in-progress", mocks.fetchInProgress],
    ["library", mocks.fetchLibraryIndex],
  ])("skips when the Plex %s view is unavailable", async (_name, fetcher) => {
    fetcher.mockResolvedValue({ status: "unavailable", reason: "Plex offline" });
    await expect(runRecommendationPipeline(logger)).resolves.toBe(
      "skipped: Plex offline",
    );
    expect(mocks.fetchCandidateBuckets).not.toHaveBeenCalled();
  });
});
