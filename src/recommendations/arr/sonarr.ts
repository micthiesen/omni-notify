import type { MediaItem, WatchlistAddOutcome } from "../types.js";
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
  titleSlug?: string;
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
          titleSlug: optionalString(series.titleSlug),
        },
      ];
    }),
  };
}

// The titleSlug on outcomes is always read from Sonarr's own series list (the
// slug is Sonarr-generated), never derived from lookup data or the local title.
export async function addSonarrSeries(
  config: ArrConfig,
  tmdbId: number,
  fetchImpl?: FetchImplementation,
): Promise<WatchlistAddOutcome> {
  if (!isConfigured(config)) return { result: "unavailable" };
  const existing = await fetchSonarrSeries(config, fetchImpl);
  if (existing.status !== "ok") return { result: "unavailable" };
  const trackedByTmdb = existing.value.find(
    (series) => series.externalIds?.tmdb === tmdbId,
  );
  if (trackedByTmdb) {
    return { result: "already_exists", titleSlug: trackedByTmdb.titleSlug };
  }

  const lookup = await requestJson<unknown>(
    config,
    `series/lookup?term=${encodeURIComponent(`tmdb:${tmdbId}`)}`,
    {},
    fetchImpl,
  );
  if (lookup.status === "unavailable") return { result: "unavailable" };
  if (lookup.status !== "ok") return { result: "error" };
  if (!Array.isArray(lookup.value) || lookup.value.length === 0) {
    return { result: "not_found" };
  }
  const series = lookup.value[0] as SonarrSeries;
  if (!optionalString(series.title) || !optionalNumber(series.tvdbId)) {
    return { result: "not_found" };
  }
  const trackedByTvdb = existing.value.find(
    (item) => item.externalIds?.tvdb === series.tvdbId,
  );
  if (trackedByTvdb) {
    return { result: "already_exists", titleSlug: trackedByTvdb.titleSlug };
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
  if (added.status === "unavailable") return { result: "unavailable" };
  if (added.status !== "ok") return { result: "error" };

  const verified = await fetchSonarrSeries(config, fetchImpl);
  if (verified.status !== "ok") return { result: "unavailable" };
  const written = verified.value.find(
    (item) => item.externalIds?.tvdb === series.tvdbId,
  );
  return written
    ? { result: "added", titleSlug: written.titleSlug }
    : { result: "error" };
}
