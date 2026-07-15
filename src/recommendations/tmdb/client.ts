import got from "got";
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

const BASE_URL = "https://api.themoviedb.org/3";

function apiKey(): string {
  const key = config.TMDB_API_KEY;
  if (!key) throw new Error("TMDB_API_KEY is not configured");
  return key;
}

async function tmdbGet<T>(
  path: string,
  schema: z.ZodType<T>,
  searchParams: Record<string, string | number | boolean> = {},
): Promise<T> {
  const key = apiKey();
  // v4 read access tokens are JWTs; v3 keys go in the query string.
  const isBearer = key.startsWith("eyJ");
  const raw = await got
    .get(`${BASE_URL}${path}`, {
      searchParams: isBearer ? searchParams : { ...searchParams, api_key: key },
      headers: isBearer ? { Authorization: `Bearer ${key}` } : {},
      timeout: { request: 15_000 },
      retry: { limit: 2 },
    })
    .json<unknown>();
  return schema.parse(raw);
}

export async function searchTitles(
  query: string,
  mediaType: MediaType,
  year?: number,
): Promise<TmdbTitle[]> {
  if (mediaType === MediaType.Movie) {
    const params: Record<string, string | number | boolean> = {
      query,
      include_adult: false,
    };
    if (year) params.year = year;
    const data = await tmdbGet("/search/movie", movieListSchema, params);
    return data.results.filter((r) => !r.adult).map(normalizeMovie);
  }
  const params: Record<string, string | number | boolean> = {
    query,
    include_adult: false,
  };
  if (year) params.first_air_date_year = year;
  const data = await tmdbGet("/search/tv", tvListSchema, params);
  return data.results.filter((r) => !r.adult).map(normalizeTv);
}

export async function findByExternalId(
  externalId: string,
  source: "imdb_id" | "tvdb_id",
): Promise<TmdbTitle[]> {
  const data = await tmdbGet(`/find/${externalId}`, findResponseSchema, {
    external_source: source,
  });
  return [
    ...data.movie_results.map(normalizeMovie),
    ...data.tv_results.map(normalizeTv),
  ];
}

export async function fetchRecommendationsFor(
  mediaType: MediaType,
  tmdbId: number,
): Promise<TmdbTitle[]> {
  if (mediaType === MediaType.Movie) {
    const data = await tmdbGet(`/movie/${tmdbId}/recommendations`, movieListSchema);
    return data.results.filter((r) => !r.adult).map(normalizeMovie);
  }
  const data = await tmdbGet(`/tv/${tmdbId}/recommendations`, tvListSchema);
  return data.results.filter((r) => !r.adult).map(normalizeTv);
}

export interface DiscoverOptions {
  withGenres?: number[];
  withoutGenres?: number[];
  minVoteCount?: number;
  page?: number;
}

export async function discoverTitles(
  mediaType: MediaType,
  options: DiscoverOptions = {},
): Promise<TmdbTitle[]> {
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
  if (mediaType === MediaType.Movie) {
    const data = await tmdbGet("/discover/movie", movieListSchema, params);
    return data.results.filter((r) => !r.adult).map(normalizeMovie);
  }
  const data = await tmdbGet("/discover/tv", tvListSchema, params);
  return data.results.filter((r) => !r.adult).map(normalizeTv);
}

export async function fetchTrending(): Promise<TmdbTitle[]> {
  const data = await tmdbGet("/trending/all/week", trendingListSchema);
  const titles: TmdbTitle[] = [];
  for (const result of data.results) {
    if ("media_type" in result && result.media_type === "movie" && "title" in result) {
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
}

export async function fetchTitleGenreIds(
  mediaType: MediaType,
  tmdbId: number,
): Promise<number[]> {
  const data = await tmdbGet(`/${mediaType}/${tmdbId}`, detailsSchema);
  return data.genres.map((g) => g.id);
}

export async function fetchTitleDetails(
  mediaType: MediaType,
  tmdbId: number,
): Promise<TmdbTitleDetails> {
  if (mediaType === MediaType.Movie) {
    const data = await tmdbGet(`/movie/${tmdbId}`, movieDetailsSchema, {
      append_to_response: "credits,keywords,release_dates",
    });
    return normalizeMovieDetails(data);
  }
  const data = await tmdbGet(`/tv/${tmdbId}`, tvDetailsSchema, {
    append_to_response: "credits,keywords,content_ratings",
  });
  return normalizeTvDetails(data);
}

const genreCache = new Map<MediaType, Map<number, string>>();

export async function getGenreMap(mediaType: MediaType): Promise<Map<number, string>> {
  const cached = genreCache.get(mediaType);
  if (cached) return cached;
  const path = mediaType === MediaType.Movie ? "/genre/movie/list" : "/genre/tv/list";
  const data = await tmdbGet(path, genreListSchema);
  const map = new Map(data.genres.map((g) => [g.id, g.name]));
  genreCache.set(mediaType, map);
  return map;
}

export function getTmdbUrl(mediaType: MediaType, tmdbId: number): string {
  return `https://www.themoviedb.org/${mediaType}/${tmdbId}`;
}
