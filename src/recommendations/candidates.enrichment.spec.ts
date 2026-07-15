import type { Logger } from "@micthiesen/mitools/logging";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { enrichCandidates, type PooledCandidate } from "./candidates.js";
import { CandidateSource, MediaType, makeCanonicalId } from "./types.js";

const mocks = vi.hoisted(() => ({
  fetchTitleDetails: vi.fn(),
  getGenreMap: vi.fn(),
}));

vi.mock("./tmdb/client.js", () => ({
  discoverTitles: vi.fn(),
  fetchRecommendationsFor: vi.fn(),
  fetchTitleDetails: mocks.fetchTitleDetails,
  fetchTrending: vi.fn(),
  getGenreMap: mocks.getGenreMap,
}));

function candidate(tmdbId: number): PooledCandidate {
  return {
    canonicalId: makeCanonicalId(MediaType.Movie, tmdbId),
    tmdbId,
    mediaType: MediaType.Movie,
    title: `Movie ${tmdbId}`,
    overview: "",
    genreIds: [18],
    voteAverage: 7,
    voteCount: 500,
    popularity: 5,
    source: CandidateSource.Similar,
  };
}

describe("enrichCandidates", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.getGenreMap.mockImplementation(async () => new Map([[18, "Drama"]]));
  });

  it("keeps a candidate when its detail request fails", async () => {
    mocks.fetchTitleDetails
      .mockResolvedValueOnce({
        runtimeMinutes: 110,
        originCountries: ["US"],
        creators: [],
        cast: [],
        keywords: [],
      })
      .mockRejectedValueOnce(new Error("TMDB unavailable"));
    const logger = { warn: vi.fn() } as unknown as Logger;

    const enriched = await enrichCandidates(
      [candidate(1), candidate(2)],
      new Set(["tmdb:movie:1"]),
      logger,
    );

    expect(enriched).toHaveLength(2);
    expect(enriched[0]).toMatchObject({
      runtimeMinutes: 110,
      genres: ["Drama"],
      inLibrary: true,
    });
    expect(enriched[1]).toMatchObject({ genres: ["Drama"], inLibrary: false });
    expect(logger.warn).toHaveBeenCalledWith(
      "TMDB details fetch failed for tmdb:movie:2",
      "TMDB unavailable",
    );
  });

  it("bounds concurrent detail requests", async () => {
    let active = 0;
    let maximumActive = 0;
    mocks.fetchTitleDetails.mockImplementation(async () => {
      active++;
      maximumActive = Math.max(maximumActive, active);
      await new Promise((resolve) => setTimeout(resolve, 1));
      active--;
      return {
        originCountries: [],
        creators: [],
        cast: [],
        keywords: [],
      };
    });

    await enrichCandidates(
      Array.from({ length: 12 }, (_, index) => candidate(index + 1)),
      new Set(),
    );

    expect(maximumActive).toBeLessThanOrEqual(6);
    expect(maximumActive).toBeGreaterThan(1);
  });
});
