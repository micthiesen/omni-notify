import got from "got";
import { Duration, Effect, Schedule, Schema } from "effect";
import type { z } from "zod";
import config from "../../utils/config.js";
import { MediaType } from "../types.js";
import {
  detailsSchema,
  findResponseSchema,
  genreListSchema,
  movieDetailsSchema,
  movieListSchema,
  normalizeMovie,
  normalizeMovieDetails,
  normalizeTv,
  normalizeTvDetails,
  type TmdbTitle,
  type TmdbTitleDetails,
  trendingListSchema,
  tvDetailsSchema,
  tvListSchema,
} from "./types.js";
import { integrationEffect, RecommendationIntegrationError } from "../effect.js";

const BASE_URL = "https://api.themoviedb.org/3";

function apiKeyEffect(): Effect.Effect<string, RecommendationIntegrationError> {
  const key = config.TMDB_API_KEY;
  return key
    ? Effect.succeed(key)
    : Effect.fail(
        new RecommendationIntegrationError({
          operation: "read TMDB API key",
          cause: new Error("TMDB_API_KEY is not configured"),
        }),
      );
}

function tmdbGet<T>(
  path: string,
  schema: z.ZodType<T>,
  searchParams: Record<string, string | number | boolean> = {},
): Effect.Effect<T, RecommendationIntegrationError> {
  return Effect.gen(function* () {
    const key = yield* apiKeyEffect();
    // v4 read access tokens are JWTs; v3 keys go in the query string.
    const isBearer = key.startsWith("eyJ");
    const raw = yield* integrationEffect(`TMDB GET ${path}`, (signal) =>
      got
        .get(`${BASE_URL}${path}`, {
          searchParams: isBearer ? searchParams : { ...searchParams, api_key: key },
          headers: isBearer ? { Authorization: `Bearer ${key}` } : {},
          timeout: { request: 15_000 },
          retry: { limit: 0 },
          signal,
        })
        .json<unknown>(),
    ).pipe(
      Effect.retry({
        schedule: Schedule.exponential(Duration.millis(200)),
        times: 2,
      }),
    );
    const decoded = yield* Effect.try({
      try: () => schema.parse(Schema.decodeUnknownSync(Schema.Unknown)(raw)),
      catch: (cause) =>
        new RecommendationIntegrationError({
          operation: `decode TMDB ${path}`,
          cause,
        }),
    });
    return decoded;
  });
}

export function searchTitlesEffect(
  query: string,
  mediaType: MediaType,
  year?: number,
): Effect.Effect<TmdbTitle[], RecommendationIntegrationError> {
  if (mediaType === MediaType.Movie) {
    const params: Record<string, string | number | boolean> = {
      query,
      include_adult: false,
    };
    if (year) params.year = year;
    return tmdbGet("/search/movie", movieListSchema, params).pipe(
      Effect.map((data) => data.results.filter((r) => !r.adult).map(normalizeMovie)),
    );
  }
  const params: Record<string, string | number | boolean> = {
    query,
    include_adult: false,
  };
  if (year) params.first_air_date_year = year;
  return tmdbGet("/search/tv", tvListSchema, params).pipe(
    Effect.map((data) => data.results.filter((r) => !r.adult).map(normalizeTv)),
  );
}

export function findByExternalIdEffect(
  externalId: string,
  source: "imdb_id" | "tvdb_id",
): Effect.Effect<TmdbTitle[], RecommendationIntegrationError> {
  return tmdbGet(`/find/${externalId}`, findResponseSchema, {
    external_source: source,
  }).pipe(
    Effect.map((data) => [
      ...data.movie_results.map(normalizeMovie),
      ...data.tv_results.map(normalizeTv),
    ]),
  );
}

export function fetchRecommendationsForEffect(
  mediaType: MediaType,
  tmdbId: number,
): Effect.Effect<TmdbTitle[], RecommendationIntegrationError> {
  if (mediaType === MediaType.Movie) {
    return tmdbGet(`/movie/${tmdbId}/recommendations`, movieListSchema).pipe(
      Effect.map((data) => data.results.filter((r) => !r.adult).map(normalizeMovie)),
    );
  }
  return tmdbGet(`/tv/${tmdbId}/recommendations`, tvListSchema).pipe(
    Effect.map((data) => data.results.filter((r) => !r.adult).map(normalizeTv)),
  );
}

export interface DiscoverOptions {
  withGenres?: number[];
  withoutGenres?: number[];
  withOriginalLanguage?: string;
  minVoteCount?: number;
  page?: number;
}

export function discoverTitlesEffect(
  mediaType: MediaType,
  options: DiscoverOptions = {},
): Effect.Effect<TmdbTitle[], RecommendationIntegrationError> {
  const params: Record<string, string | number | boolean> = {
    include_adult: false,
    sort_by: "vote_average.desc",
    "vote_count.gte": options.minVoteCount ?? 300,
    page: options.page ?? 1,
  };
  if (options.withGenres?.length) params.with_genres = options.withGenres.join(",");
  if (options.withoutGenres?.length) {
    params.without_genres = options.withoutGenres.join(",");
  }
  if (options.withOriginalLanguage) {
    params.with_original_language = options.withOriginalLanguage;
  }
  if (mediaType === MediaType.Movie) {
    return tmdbGet("/discover/movie", movieListSchema, params).pipe(
      Effect.map((data) => data.results.filter((r) => !r.adult).map(normalizeMovie)),
    );
  }
  return tmdbGet("/discover/tv", tvListSchema, params).pipe(
    Effect.map((data) => data.results.filter((r) => !r.adult).map(normalizeTv)),
  );
}

export function fetchTrendingEffect(): Effect.Effect<
  TmdbTitle[],
  RecommendationIntegrationError
> {
  return tmdbGet("/trending/all/week", trendingListSchema).pipe(
    Effect.map((data) => {
      const titles: TmdbTitle[] = [];
      for (const result of data.results) {
        if (
          "media_type" in result &&
          result.media_type === "movie" &&
          "title" in result
        ) {
          if (!result.adult) titles.push(normalizeMovie(result));
        } else if (
          "media_type" in result &&
          result.media_type === "tv" &&
          "name" in result
        ) {
          if (!result.adult) titles.push(normalizeTv(result));
        }
      }
      return titles;
    }),
  );
}

export function fetchTitleGenreIdsEffect(
  mediaType: MediaType,
  tmdbId: number,
): Effect.Effect<number[], RecommendationIntegrationError> {
  return tmdbGet(`/${mediaType}/${tmdbId}`, detailsSchema).pipe(
    Effect.map((data) => data.genres.map((g) => g.id)),
  );
}

export function fetchTitleDetailsEffect(
  mediaType: MediaType,
  tmdbId: number,
): Effect.Effect<TmdbTitleDetails, RecommendationIntegrationError> {
  if (mediaType === MediaType.Movie) {
    return tmdbGet(`/movie/${tmdbId}`, movieDetailsSchema, {
      append_to_response: "credits,keywords,release_dates",
    }).pipe(Effect.map(normalizeMovieDetails));
  }
  return tmdbGet(`/tv/${tmdbId}`, tvDetailsSchema, {
    append_to_response: "credits,keywords,content_ratings",
  }).pipe(Effect.map(normalizeTvDetails));
}

const genreCache = new Map<MediaType, Map<number, string>>();

export function getGenreMapEffect(
  mediaType: MediaType,
): Effect.Effect<Map<number, string>, RecommendationIntegrationError> {
  const cached = genreCache.get(mediaType);
  if (cached) return Effect.succeed(cached);
  const path = mediaType === MediaType.Movie ? "/genre/movie/list" : "/genre/tv/list";
  return tmdbGet(path, genreListSchema).pipe(
    Effect.map((data) => {
      const map = new Map(data.genres.map((g) => [g.id, g.name]));
      genreCache.set(mediaType, map);
      return map;
    }),
  );
}

export function getTmdbUrl(mediaType: MediaType, tmdbId: number): string {
  return `https://www.themoviedb.org/${mediaType}/${tmdbId}`;
}
