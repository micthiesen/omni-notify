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

interface SonarrSeries extends Record<string, unknown> {
  id?: number;
  title?: string;
  year?: number;
  tvdbId?: number;
  tmdbId?: number;
  imdbId?: string;
}

export async function fetchSonarrSeries(
  config: ArrConfig,
  fetchImpl?: FetchImplementation,
): Promise<{ status: "ok"; value: MediaItem[] } | { status: "unavailable" }> {
  if (!hasArrConnection(config)) return { status: "unavailable" };
  const response = await requestJson<unknown>(config, "series", {}, fetchImpl);
  if (response.status !== "ok" || !Array.isArray(response.value)) {
    return { status: "unavailable" };
  }
  return {
    status: "ok",
    value: response.value.flatMap((raw) => {
      const series = raw as SonarrSeries;
      const id = optionalNumber(series.id);
      const title = optionalString(series.title);
      const tvdb = optionalNumber(series.tvdbId);
      if (id === undefined || title === undefined || tvdb === undefined) return [];
      return [
        {
          guid: `sonarr:${id}`,
          title,
          year: optionalNumber(series.year),
          mediaType: MediaType.Tv,
          externalIds: {
            tvdb,
            tmdb: optionalNumber(series.tmdbId),
            imdb: optionalString(series.imdbId),
          },
        },
      ];
    }),
  };
}

export async function addSonarrSeries(
  config: ArrConfig,
  tmdbId: number,
  fetchImpl?: FetchImplementation,
): Promise<AddToWatchlistResult> {
  if (!isConfigured(config)) return "unavailable";
  const existing = await fetchSonarrSeries(config, fetchImpl);
  if (existing.status !== "ok") return "unavailable";
  if (existing.value.some((series) => series.externalIds?.tmdb === tmdbId)) {
    return "already_exists";
  }

  const lookup = await requestJson<unknown>(
    config,
    `series/lookup?term=${encodeURIComponent(`tmdb:${tmdbId}`)}`,
    {},
    fetchImpl,
  );
  if (lookup.status === "unavailable") return "unavailable";
  if (lookup.status !== "ok") return "error";
  if (!Array.isArray(lookup.value) || lookup.value.length === 0) return "not_found";
  const series = lookup.value[0] as SonarrSeries;
  if (!optionalString(series.title) || !optionalNumber(series.tvdbId)) {
    return "not_found";
  }
  if (existing.value.some((item) => item.externalIds?.tvdb === series.tvdbId)) {
    return "already_exists";
  }

  const added = await requestJson<unknown>(
    config,
    "series",
    postJson({
      ...series,
      qualityProfileId: config.qualityProfileId,
      rootFolderPath: config.rootFolderPath,
      monitored: true,
      seasonFolder: true,
      addOptions: { searchForMissingEpisodes: true },
    }),
    fetchImpl,
  );
  if (added.status === "unavailable") return "unavailable";
  if (added.status !== "ok") return "error";

  const verified = await fetchSonarrSeries(config, fetchImpl);
  if (verified.status !== "ok") return "unavailable";
  return verified.value.some((item) => item.externalIds?.tvdb === series.tvdbId)
    ? "added"
    : "error";
}
