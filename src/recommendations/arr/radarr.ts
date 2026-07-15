import type { AddToWatchlistResult, MediaItem } from "../types.js";
import { MediaType } from "../types.js";
import {
  type ArrConfig,
  type FetchImplementation,
  hasArrConnection,
  isConfigured,
  optionalNumber,
  optionalString,
  postJson,
  requestJson,
} from "./client.js";

interface RadarrMovie extends Record<string, unknown> {
  id?: number;
  title?: string;
  year?: number;
  tmdbId?: number;
  imdbId?: string;
}

export async function fetchRadarrMovies(
  config: ArrConfig,
  fetchImpl?: FetchImplementation,
): Promise<{ status: "ok"; value: MediaItem[] } | { status: "unavailable" }> {
  if (!hasArrConnection(config)) return { status: "unavailable" };
  const response = await requestJson<unknown>(config, "movie", {}, fetchImpl);
  if (response.status !== "ok" || !Array.isArray(response.value)) {
    return { status: "unavailable" };
  }
  return {
    status: "ok",
    value: response.value.flatMap((raw) => {
      const movie = raw as RadarrMovie;
      const id = optionalNumber(movie.id);
      const title = optionalString(movie.title);
      const tmdb = optionalNumber(movie.tmdbId);
      if (id === undefined || title === undefined || tmdb === undefined) return [];
      return [
        {
          guid: `radarr:${id}`,
          title,
          year: optionalNumber(movie.year),
          mediaType: MediaType.Movie,
          externalIds: { tmdb, imdb: optionalString(movie.imdbId) },
        },
      ];
    }),
  };
}

export async function addRadarrMovie(
  config: ArrConfig,
  tmdbId: number,
  fetchImpl?: FetchImplementation,
): Promise<AddToWatchlistResult> {
  if (!isConfigured(config)) return "unavailable";
  const existing = await fetchRadarrMovies(config, fetchImpl);
  if (existing.status !== "ok") return "unavailable";
  if (existing.value.some((movie) => movie.externalIds?.tmdb === tmdbId)) {
    return "already_exists";
  }

  const lookup = await requestJson<RadarrMovie>(
    config,
    `movie/lookup/tmdb?tmdbId=${encodeURIComponent(tmdbId)}`,
    {},
    fetchImpl,
  );
  if (lookup.status === "unavailable") return "unavailable";
  if (lookup.status !== "ok") return "error";
  if (!optionalString(lookup.value.title) || !optionalNumber(lookup.value.tmdbId)) {
    return "not_found";
  }

  const added = await requestJson<unknown>(
    config,
    "movie",
    postJson({
      ...lookup.value,
      qualityProfileId: config.qualityProfileId,
      rootFolderPath: config.rootFolderPath,
      monitored: true,
      addOptions: { searchForMovie: true },
    }),
    fetchImpl,
  );
  if (added.status === "unavailable") return "unavailable";
  if (added.status !== "ok") return "error";

  const verified = await fetchRadarrMovies(config, fetchImpl);
  if (verified.status !== "ok") return "unavailable";
  return verified.value.some((movie) => movie.externalIds?.tmdb === tmdbId)
    ? "added"
    : "error";
}
