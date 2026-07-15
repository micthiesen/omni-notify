import { z } from "zod";
import { MediaType } from "../types.js";

/** Normalized TMDB list-payload title (movie or TV). */
export interface TmdbTitle {
  tmdbId: number;
  mediaType: MediaType;
  title: string;
  year?: number;
  overview: string;
  genreIds: number[];
  voteAverage: number;
  voteCount: number;
  popularity: number;
  posterPath?: string;
}

/** Structured title details used to judge fit and viewing commitment. */
export interface TmdbTitleDetails {
  genres: string[];
  runtimeMinutes?: number;
  seasonCount?: number;
  episodeCount?: number;
  seriesStatus?: string;
  originalLanguage?: string;
  originCountries: string[];
  creators: string[];
  cast: string[];
  keywords: string[];
  certification?: string;
}

const movieResultSchema = z.object({
  id: z.number(),
  title: z.string(),
  release_date: z.string().optional(),
  overview: z.string().optional().default(""),
  genre_ids: z.array(z.number()).optional().default([]),
  vote_average: z.number().optional().default(0),
  vote_count: z.number().optional().default(0),
  popularity: z.number().optional().default(0),
  poster_path: z.string().nullable().optional(),
  adult: z.boolean().optional().default(false),
});

const tvResultSchema = z.object({
  id: z.number(),
  name: z.string(),
  first_air_date: z.string().optional(),
  overview: z.string().optional().default(""),
  genre_ids: z.array(z.number()).optional().default([]),
  vote_average: z.number().optional().default(0),
  vote_count: z.number().optional().default(0),
  popularity: z.number().optional().default(0),
  poster_path: z.string().nullable().optional(),
  adult: z.boolean().optional().default(false),
});

export type MovieResult = z.infer<typeof movieResultSchema>;
export type TvResult = z.infer<typeof tvResultSchema>;

export const movieListSchema = z.object({ results: z.array(movieResultSchema) });
export const tvListSchema = z.object({ results: z.array(tvResultSchema) });

// Trending "all" mixes movies, TV, and people; media_type discriminates.
const trendingMovieSchema = movieResultSchema.extend({
  media_type: z.literal("movie"),
});
const trendingTvSchema = tvResultSchema.extend({ media_type: z.literal("tv") });
const trendingOtherSchema = z.object({ media_type: z.string() });

export const trendingListSchema = z.object({
  results: z.array(
    z.union([trendingMovieSchema, trendingTvSchema, trendingOtherSchema]),
  ),
});

export const findResponseSchema = z.object({
  movie_results: z.array(movieResultSchema).optional().default([]),
  tv_results: z.array(tvResultSchema).optional().default([]),
});

const namedPersonSchema = z.object({ name: z.string() });
const keywordSchema = z.object({ name: z.string() });
const certificationSchema = z.object({ certification: z.string() });

export const detailsSchema = z.object({
  genres: z.array(z.object({ id: z.number(), name: z.string() })),
});

export const movieDetailsSchema = detailsSchema.extend({
  runtime: z.number().nullable().optional(),
  original_language: z.string().optional(),
  origin_country: z.array(z.string()).optional().default([]),
  credits: z
    .object({
      cast: z.array(namedPersonSchema).optional().default([]),
      crew: z
        .array(namedPersonSchema.extend({ job: z.string().optional() }))
        .optional()
        .default([]),
    })
    .optional(),
  keywords: z
    .object({ keywords: z.array(keywordSchema).optional().default([]) })
    .optional(),
  release_dates: z
    .object({
      results: z
        .array(
          z.object({
            iso_3166_1: z.string(),
            release_dates: z.array(certificationSchema).optional().default([]),
          }),
        )
        .optional()
        .default([]),
    })
    .optional(),
});

export const tvDetailsSchema = detailsSchema.extend({
  episode_run_time: z.array(z.number()).optional().default([]),
  number_of_seasons: z.number().optional(),
  number_of_episodes: z.number().optional(),
  status: z.string().optional(),
  original_language: z.string().optional(),
  origin_country: z.array(z.string()).optional().default([]),
  created_by: z.array(namedPersonSchema).optional().default([]),
  credits: z
    .object({ cast: z.array(namedPersonSchema).optional().default([]) })
    .optional(),
  keywords: z
    .object({ results: z.array(keywordSchema).optional().default([]) })
    .optional(),
  content_ratings: z
    .object({
      results: z
        .array(z.object({ iso_3166_1: z.string(), rating: z.string() }))
        .optional()
        .default([]),
    })
    .optional(),
});

export const genreListSchema = z.object({
  genres: z.array(z.object({ id: z.number(), name: z.string() })),
});

function parseYear(date: string | undefined): number | undefined {
  if (!date) return undefined;
  const year = Number(date.slice(0, 4));
  return Number.isFinite(year) && year > 1800 ? year : undefined;
}

export function normalizeMovie(result: MovieResult): TmdbTitle {
  return {
    tmdbId: result.id,
    mediaType: MediaType.Movie,
    title: result.title,
    year: parseYear(result.release_date),
    overview: result.overview,
    genreIds: result.genre_ids,
    voteAverage: result.vote_average,
    voteCount: result.vote_count,
    popularity: result.popularity,
    posterPath: result.poster_path ?? undefined,
  };
}

export function normalizeTv(result: TvResult): TmdbTitle {
  return {
    tmdbId: result.id,
    mediaType: MediaType.Tv,
    title: result.name,
    year: parseYear(result.first_air_date),
    overview: result.overview,
    genreIds: result.genre_ids,
    voteAverage: result.vote_average,
    voteCount: result.vote_count,
    popularity: result.popularity,
    posterPath: result.poster_path ?? undefined,
  };
}

function nonEmpty(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

export function normalizeMovieDetails(
  result: z.infer<typeof movieDetailsSchema>,
): TmdbTitleDetails {
  const usReleases = result.release_dates?.results.find(
    (entry) => entry.iso_3166_1 === "US",
  )?.release_dates;
  return {
    genres: result.genres.map((genre) => genre.name),
    runtimeMinutes:
      result.runtime !== null && result.runtime !== undefined && result.runtime > 0
        ? result.runtime
        : undefined,
    originalLanguage: nonEmpty(result.original_language),
    originCountries: result.origin_country,
    creators:
      result.credits?.crew
        .filter((person) => person.job === "Director")
        .map((person) => person.name)
        .slice(0, 3) ?? [],
    cast: result.credits?.cast.map((person) => person.name).slice(0, 6) ?? [],
    keywords:
      result.keywords?.keywords.map((keyword) => keyword.name).slice(0, 12) ?? [],
    certification: nonEmpty(
      usReleases?.find((release) => nonEmpty(release.certification))?.certification,
    ),
  };
}

export function normalizeTvDetails(
  result: z.infer<typeof tvDetailsSchema>,
): TmdbTitleDetails {
  const runtimes = result.episode_run_time.filter((runtime) => runtime > 0);
  const sortedRuntimes = [...runtimes].sort((a, b) => a - b);
  const typicalRuntime = sortedRuntimes[Math.floor(sortedRuntimes.length / 2)];
  return {
    genres: result.genres.map((genre) => genre.name),
    runtimeMinutes: typicalRuntime,
    seasonCount: result.number_of_seasons,
    episodeCount: result.number_of_episodes,
    seriesStatus: nonEmpty(result.status),
    originalLanguage: nonEmpty(result.original_language),
    originCountries: result.origin_country,
    creators: result.created_by.map((person) => person.name).slice(0, 3),
    cast: result.credits?.cast.map((person) => person.name).slice(0, 6) ?? [],
    keywords:
      result.keywords?.results.map((keyword) => keyword.name).slice(0, 12) ?? [],
    certification: nonEmpty(
      result.content_ratings?.results.find((entry) => entry.iso_3166_1 === "US")
        ?.rating,
    ),
  };
}
