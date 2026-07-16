import type { Logger } from "@micthiesen/mitools/logging";
import PQueue from "p-queue";
import {
  discoverTitles,
  fetchRecommendationsFor,
  fetchTitleDetails,
  fetchTrending,
  getGenreMap,
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
export async function fetchCandidateBuckets(
  seeds: WatchSeed[],
  logger: Logger,
): Promise<SourceBucket[]> {
  const recentSeeds = seeds.slice(0, SEED_LIMIT);
  const topGenres = rankGenres(seeds).slice(0, 3);

  const [similar, discover, trending, novelty] = await Promise.all([
    fetchSimilarBucket(recentSeeds, logger),
    fetchDiscoverBucket(topGenres, logger),
    fetchTrending().catch((error) => {
      logger.warn("TMDB trending fetch failed", (error as Error).message);
      return [];
    }),
    fetchNoveltyBucket(topGenres, logger),
  ]);

  return [
    { source: CandidateSource.Similar, titles: englishOnly(similar) },
    { source: CandidateSource.Discover, titles: englishOnly(discover) },
    { source: CandidateSource.Trending, titles: englishOnly(trending) },
    { source: CandidateSource.Novelty, titles: englishOnly(novelty) },
  ];
}

async function fetchSimilarBucket(
  seeds: WatchSeed[],
  logger: Logger,
): Promise<TmdbTitle[]> {
  const results = await Promise.all(
    seeds.map((seed) =>
      fetchRecommendationsFor(seed.mediaType, seed.tmdbId).catch((error) => {
        logger.warn(
          `TMDB recommendations fetch failed for ${seed.canonicalId}`,
          (error as Error).message,
        );
        return [];
      }),
    ),
  );
  // Interleave per-seed results so one seed can't dominate the bucket.
  return interleave(
    results.map((r) => r.filter(isEligibleOriginalLanguage).slice(0, 12)),
  );
}

async function fetchDiscoverBucket(
  topGenres: number[],
  logger: Logger,
): Promise<TmdbTitle[]> {
  if (topGenres.length === 0) return [];
  const results = await Promise.all(
    [MediaType.Movie, MediaType.Tv].map((mediaType) =>
      discoverTitles(mediaType, {
        withGenres: topGenres,
        withOriginalLanguage: REQUIRED_ORIGINAL_LANGUAGE,
      }).catch((error) => {
        logger.warn(`TMDB discover failed (${mediaType})`, (error as Error).message);
        return [];
      }),
    ),
  );
  return interleave(results);
}

async function fetchNoveltyBucket(
  topGenres: number[],
  logger: Logger,
): Promise<TmdbTitle[]> {
  const results = await Promise.all(
    [MediaType.Movie, MediaType.Tv].map((mediaType) =>
      discoverTitles(mediaType, {
        withoutGenres: topGenres,
        withOriginalLanguage: REQUIRED_ORIGINAL_LANGUAGE,
        minVoteCount: 1000,
      }).catch((error) => {
        logger.warn(
          `TMDB novelty discover failed (${mediaType})`,
          (error as Error).message,
        );
        return [];
      }),
    ),
  );
  return interleave(results);
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
export async function enrichCandidates(
  pool: PooledCandidate[],
  libraryIds: Set<string>,
  logger?: Logger,
): Promise<Candidate[]> {
  const [movieGenres, tvGenres] = await Promise.all([
    getGenreMap(MediaType.Movie),
    getGenreMap(MediaType.Tv),
  ]);
  const detailsQueue = new PQueue({ concurrency: 6 });
  const details = await Promise.all(
    pool.map((candidate) =>
      detailsQueue.add(async () => {
        try {
          return await fetchTitleDetails(candidate.mediaType, candidate.tmdbId);
        } catch (error) {
          logger?.warn(
            `TMDB details fetch failed for ${candidate.canonicalId}`,
            (error as Error).message,
          );
          return undefined;
        }
      }),
    ),
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
