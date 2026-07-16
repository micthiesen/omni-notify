import { describe, expect, it } from "vitest";
import {
  assemblePool,
  MAX_SOURCE_SHARE,
  rankGenres,
  type SourceBucket,
} from "./candidates.js";
import type { TmdbTitle } from "./tmdb/types.js";
import { CandidateSource, MediaType } from "./types.js";

function makeTitle(tmdbId: number, mediaType = MediaType.Movie): TmdbTitle {
  return {
    tmdbId,
    mediaType,
    title: `Title ${tmdbId}`,
    overview: "",
    genreIds: [],
    voteAverage: 7,
    voteCount: 500,
    popularity: 5,
    originalLanguage: "en",
  };
}

function bucket(source: CandidateSource, ids: number[]): SourceBucket {
  return { source, titles: ids.map((id) => makeTitle(id)) };
}

describe("assemblePool", () => {
  it("dedupes across buckets, first source wins", () => {
    const pool = assemblePool(
      [
        bucket(CandidateSource.Similar, [1, 2, 3]),
        bucket(CandidateSource.Trending, [2, 3, 4]),
      ],
      20,
    );
    const ids = pool.map((c) => c.tmdbId);
    expect(ids).toEqual([1, 2, 3, 4]);
    expect(pool.find((c) => c.tmdbId === 2)?.source).toBe(CandidateSource.Similar);
  });

  it("dedupes same tmdb id across media types as distinct candidates", () => {
    const pool = assemblePool(
      [
        {
          source: CandidateSource.Similar,
          titles: [makeTitle(1, MediaType.Movie), makeTitle(1, MediaType.Tv)],
        },
      ],
      20,
    );
    expect(pool).toHaveLength(2);
  });

  it("caps each source at the max share", () => {
    const target = 30;
    const cap = Math.ceil(target * MAX_SOURCE_SHARE);
    const pool = assemblePool(
      [
        bucket(
          CandidateSource.Similar,
          Array.from({ length: 50 }, (_, i) => i + 1),
        ),
      ],
      target,
    );
    expect(pool.length).toBeLessThanOrEqual(cap);
  });

  it("reserves room for the novelty bucket", () => {
    const target = 30;
    const pool = assemblePool(
      [
        bucket(CandidateSource.Similar, range(100, 110)),
        bucket(CandidateSource.Discover, range(200, 210)),
        bucket(CandidateSource.Trending, range(300, 310)),
        bucket(CandidateSource.Novelty, range(400, 410)),
      ],
      target,
    );
    const noveltyCount = pool.filter(
      (c) => c.source === CandidateSource.Novelty,
    ).length;
    expect(noveltyCount).toBeGreaterThan(0);
    expect(pool.length).toBeLessThanOrEqual(target);
  });

  it("never exceeds the target size", () => {
    const pool = assemblePool(
      [
        bucket(CandidateSource.Similar, range(1, 40)),
        bucket(CandidateSource.Discover, range(100, 140)),
        bucket(CandidateSource.Trending, range(200, 240)),
        bucket(CandidateSource.Novelty, range(300, 340)),
      ],
      50,
    );
    expect(pool.length).toBeLessThanOrEqual(50);
  });

  it("keeps only titles whose original language is English", () => {
    const english = makeTitle(1);
    const french = { ...makeTitle(2), originalLanguage: "fr" };
    const unknown = { ...makeTitle(3), originalLanguage: undefined };

    const pool = assemblePool(
      [{ source: CandidateSource.Trending, titles: [english, french, unknown] }],
      20,
    );

    expect(pool.map((candidate) => candidate.tmdbId)).toEqual([1]);
    expect(pool[0]?.originalLanguage).toBe("en");
  });
});

describe("rankGenres", () => {
  it("ranks genres by frequency across seeds", () => {
    const ranked = rankGenres([
      seed([18, 35]),
      seed([18]),
      seed([18, 878]),
      seed([878]),
    ]);
    expect(ranked[0]).toBe(18);
    expect(ranked[1]).toBe(878);
    expect(ranked[2]).toBe(35);
  });
});

function seed(genreIds: number[]) {
  return {
    canonicalId: "tmdb:movie:1" as const,
    tmdbId: 1,
    mediaType: MediaType.Movie,
    genreIds,
  };
}

function range(from: number, to: number): number[] {
  return Array.from({ length: to - from }, (_, i) => from + i);
}
