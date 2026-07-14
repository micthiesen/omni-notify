import { describe, expect, it } from "vitest";
import type { PooledCandidate } from "./candidates.js";
import { filterEligible } from "./filters.js";
import { CandidateSource, MediaType, makeCanonicalId } from "./types.js";

function makeCandidate(tmdbId: number): PooledCandidate {
  return {
    canonicalId: makeCanonicalId(MediaType.Movie, tmdbId),
    tmdbId,
    mediaType: MediaType.Movie,
    title: `Movie ${tmdbId}`,
    overview: "",
    genreIds: [],
    voteAverage: 7,
    voteCount: 1000,
    popularity: 10,
    source: CandidateSource.Trending,
  };
}

describe("filterEligible", () => {
  it("drops watched, in-progress, watchlisted, and excluded candidates", () => {
    const pool = [1, 2, 3, 4, 5].map(makeCandidate);
    const { kept, dropped } = filterEligible(pool, {
      watchedIds: new Set(["tmdb:movie:1"]),
      inProgressIds: new Set(["tmdb:movie:2"]),
      watchlistIds: new Set(["tmdb:movie:3"]),
      excludedRecommendationIds: new Set(["tmdb:movie:4"]),
    });

    expect(kept.map((c) => c.tmdbId)).toEqual([5]);
    expect(dropped).toHaveLength(4);
    expect(dropped.map((d) => d.reason)).toEqual([
      "already watched",
      "currently in progress",
      "already on watchlist",
      "recently recommended or terminal outcome",
    ]);
  });

  it("keeps everything when context sets are empty", () => {
    const pool = [1, 2].map(makeCandidate);
    const { kept, dropped } = filterEligible(pool, {
      watchedIds: new Set(),
      inProgressIds: new Set(),
      watchlistIds: new Set(),
      excludedRecommendationIds: new Set(),
    });
    expect(kept).toHaveLength(2);
    expect(dropped).toHaveLength(0);
  });
});
