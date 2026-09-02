import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { effectMessage, RecommendationIntegrationError } from "./effect.js";
import {
  discoverTitlesEffect as discoverTitles,
  fetchRecommendationsForEffect as fetchRecommendationsFor,
  fetchTitleDetailsEffect as fetchTitleDetails,
  fetchTrendingEffect as fetchTrending,
  getGenreMapEffect as getGenreMap,
} from "./tmdb/client.js";
import type { TmdbTitle } from "./tmdb/types.js";
import {
  type Candidate,
  CandidateSource,
  type CanonicalId,
  MediaType,
  makeCanonicalId,
} from "./types.js";

export const TARGET_POOL_SIZE = 80;
/** No single source bucket may exceed this share of the pool. */
export const MAX_SOURCE_SHARE = 1 / 3;
const SEED_LIMIT = 8;
const NOVELTY_SHARE = 0.15;
const REQUIRED_ORIGINAL_LANGUAGE = "en";

export interface WatchSeed {
  canonicalId: CanonicalId;
  tmdbId: number;
  mediaType: MediaType;
  genreIds: number[];
}

export interface SourceBucket {
  source: CandidateSource;
  titles: TmdbTitle[];
}

/**
 * Fetch raw candidate buckets from TMDB: recommendations seeded by recent
 * completed watches, discover on the user's top genres, this week's trending,
 * and a novelty bucket outside the top genres.
 */
export function fetchCandidateBuckets(seeds: WatchSeed[], logger: Logger) {
  return Effect.gen(function* () {
    const recentSeeds = seeds.slice(0, SEED_LIMIT);
    const topGenres = rankGenres(seeds).slice(0, 3);
    const [similar, discover, trending, novelty] = yield* Effect.all(
      [
        fetchSimilarBucketEffect(recentSeeds, logger),
        fetchDiscoverBucketEffect(topGenres, logger),
        fetchTrending().pipe(
          Effect.catch((error) =>
            logger
              .warn("TMDB trending fetch failed", effectMessage(error))
              .pipe(Effect.as([])),
          ),
        ),
        fetchNoveltyBucketEffect(topGenres, logger),
      ] as const,
      { concurrency: "unbounded" },
    );
    return [
      { source: CandidateSource.Similar, titles: englishOnly(similar) },
      { source: CandidateSource.Discover, titles: englishOnly(discover) },
      { source: CandidateSource.Trending, titles: englishOnly(trending) },
      { source: CandidateSource.Novelty, titles: englishOnly(novelty) },
    ];
  });
}

function fetchSimilarBucketEffect(seeds: WatchSeed[], logger: Logger) {
  return Effect.forEach(
    seeds,
    (seed) =>
      fetchRecommendationsFor(seed.mediaType, seed.tmdbId).pipe(
        Effect.catch((error) =>
          logger
            .warn(
              `TMDB recommendations fetch failed for ${seed.canonicalId}`,
              effectMessage(error),
            )
            .pipe(Effect.as([])),
        ),
      ),
    { concurrency: 4 },
  ).pipe(
    Effect.map((results) =>
      interleave(results.map((r) => r.filter(isEligibleOriginalLanguage).slice(0, 12))),
    ),
  );
}

function fetchDiscoverBucketEffect(topGenres: number[], logger: Logger) {
  if (topGenres.length === 0) return Effect.succeed([]);
  return Effect.forEach(
    [MediaType.Movie, MediaType.Tv],
    (mediaType) =>
      discoverTitles(mediaType, {
        withGenres: topGenres,
        withOriginalLanguage: REQUIRED_ORIGINAL_LANGUAGE,
      }).pipe(
        Effect.catch((error) =>
          logger
            .warn(`TMDB discover failed (${mediaType})`, effectMessage(error))
            .pipe(Effect.as([])),
        ),
      ),
    { concurrency: "unbounded" },
  ).pipe(Effect.map(interleave));
}

function fetchNoveltyBucketEffect(topGenres: number[], logger: Logger) {
  return Effect.forEach(
    [MediaType.Movie, MediaType.Tv],
    (mediaType) =>
      discoverTitles(mediaType, {
        withoutGenres: topGenres,
        withOriginalLanguage: REQUIRED_ORIGINAL_LANGUAGE,
        minVoteCount: 1000,
      }).pipe(
        Effect.catch((error) =>
          logger
            .warn(`TMDB novelty discover failed (${mediaType})`, effectMessage(error))
            .pipe(Effect.as([])),
        ),
      ),
    { concurrency: "unbounded" },
  ).pipe(Effect.map(interleave));
}

export function rankGenres(seeds: WatchSeed[]): number[] {
  const counts = new Map<number, number>();
  for (const seed of seeds) {
    for (const genreId of seed.genreIds) {
      counts.set(genreId, (counts.get(genreId) ?? 0) + 1);
    }
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).map(([id]) => id);
}

/**
 * Assemble the candidate pool from source buckets: dedupe by canonical id
 * (first source wins), enforce per-source quotas, reserve a novelty share,
 * and cap the pool size. Pure for testability.
 */
export type PooledCandidate = Omit<Candidate, "genres" | "inLibrary"> & {
  genreIds: number[];
};

export function assemblePool(
  buckets: SourceBucket[],
  targetSize: number = TARGET_POOL_SIZE,
): PooledCandidate[] {
  const perSourceCap = Math.ceil(targetSize * MAX_SOURCE_SHARE);
  const noveltyReserve = Math.floor(targetSize * NOVELTY_SHARE);
  const seen = new Set<string>();
  const pool: PooledCandidate[] = [];

  const take = (bucket: SourceBucket, cap: number) => {
    let taken = 0;
    for (const title of bucket.titles) {
      if (taken >= cap || pool.length >= targetSize) break;
      if (!isEligibleOriginalLanguage(title)) continue;
      const canonicalId = makeCanonicalId(title.mediaType, title.tmdbId);
      if (seen.has(canonicalId)) continue;
      seen.add(canonicalId);
      pool.push({
        canonicalId,
        tmdbId: title.tmdbId,
        mediaType: title.mediaType,
        title: title.title,
        year: title.year,
        overview: title.overview,
        genreIds: title.genreIds,
        voteAverage: title.voteAverage,
        voteCount: title.voteCount,
        popularity: title.popularity,
        posterPath: title.posterPath,
        originalLanguage: title.originalLanguage,
        source: bucket.source,
      });
      taken++;
    }
  };

  // Fill non-novelty buckets into the shared budget, then guarantee the
  // novelty reserve on top (novelty is also subject to the per-source cap).
  const novelty = buckets.filter((b) => b.source === CandidateSource.Novelty);
  const rest = buckets.filter((b) => b.source !== CandidateSource.Novelty);
  const restBudget = targetSize - noveltyReserve;
  for (const bucket of rest) {
    take(bucket, Math.min(perSourceCap, Math.max(0, restBudget - pool.length)));
  }
  for (const bucket of novelty) {
    take(bucket, Math.min(perSourceCap, noveltyReserve));
  }

  return pool;
}

export function isEligibleOriginalLanguage(title: TmdbTitle): boolean {
  return title.originalLanguage === REQUIRED_ORIGINAL_LANGUAGE;
}

function englishOnly(titles: TmdbTitle[]): TmdbTitle[] {
  return titles.filter(isEligibleOriginalLanguage);
}

/** Attach genre names (via the TMDB genre maps) and library presence. */
export function enrichCandidates(
  pool: PooledCandidate[],
  libraryIds: Set<string>,
  logger?: Logger,
): Effect.Effect<Candidate[], RecommendationIntegrationError> {
  return Effect.gen(function* () {
    const [movieGenres, tvGenres] = yield* Effect.all(
      [getGenreMap(MediaType.Movie), getGenreMap(MediaType.Tv)] as const,
      { concurrency: "unbounded" },
    );
    const details = yield* Effect.forEach(
      pool,
      (candidate) =>
        fetchTitleDetails(candidate.mediaType, candidate.tmdbId).pipe(
          Effect.catch((error) => {
            logger?.warn(
              `TMDB details fetch failed for ${candidate.canonicalId}`,
              effectMessage(error),
            );
            return Effect.succeed(undefined);
          }),
        ),
      { concurrency: 6 },
    );
    return pool.map(({ genreIds, ...c }, index) => {
      const genreMap = c.mediaType === MediaType.Movie ? movieGenres : tvGenres;
      return {
        ...c,
        ...details[index],
        genres: genreIds
          .map((id) => genreMap.get(id))
          .filter((g): g is string => g !== undefined),
        inLibrary: libraryIds.has(c.canonicalId),
      };
    });
  });
}

function interleave<T>(lists: T[][]): T[] {
  const result: T[] = [];
  const maxLength = Math.max(0, ...lists.map((l) => l.length));
  for (let i = 0; i < maxLength; i++) {
    for (const list of lists) {
      if (i < list.length) result.push(list[i]);
    }
  }
  return result;
}
