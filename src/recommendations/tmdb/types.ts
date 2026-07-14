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

export const detailsSchema = z.object({
  genres: z.array(z.object({ id: z.number(), name: z.string() })),
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
