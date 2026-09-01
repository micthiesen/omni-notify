import type { MediaItem, WatchlistAddOutcome } from "../types.js";
import { Effect, Schema } from "effect";
import { MediaType } from "../types.js";
import {
  type ArrConfig,
  type FetchImplementation,
  hasArrConnection,
  isConfigured,
  postJson,
  requestJson,
} from "./client.js";

const SonarrSeriesSchema = Schema.Struct({
  id: Schema.optional(Schema.Number),
  title: Schema.optional(Schema.String),
  titleSlug: Schema.optional(Schema.String),
  year: Schema.optional(Schema.Number),
  tvdbId: Schema.optional(Schema.Number),
  tmdbId: Schema.optional(Schema.Number),
  imdbId: Schema.optional(Schema.String),
});
const SonarrSeriesListSchema = Schema.Array(SonarrSeriesSchema);

export function fetchSonarrSeries(
  config: ArrConfig,
  fetchImpl?: FetchImplementation,
): Effect.Effect<{ status: "ok"; value: MediaItem[] } | { status: "unavailable" }> {
  if (!hasArrConnection(config)) return Effect.succeed({ status: "unavailable" });
  return Effect.gen(function* () {
    const response = yield* requestJson(
      config,
      "series",
      SonarrSeriesListSchema,
      {},
      fetchImpl,
    );
    if (response.status !== "ok") {
      return { status: "unavailable" };
    }
    return {
      status: "ok",
      value: response.value.flatMap((series) => {
        const id = series.id;
        const title = series.title;
        const tvdb = series.tvdbId;
        if (id === undefined || title === undefined || tvdb === undefined) return [];
        return [
          {
            guid: `sonarr:${id}`,
            title,
            year: series.year,
            mediaType: MediaType.Tv,
            externalIds: {
              tvdb,
              tmdb: series.tmdbId,
              imdb: series.imdbId,
            },
            titleSlug: series.titleSlug,
          },
        ];
      }),
    };
  });
}

// The titleSlug on outcomes is always read from Sonarr's own series list (the
// slug is Sonarr-generated), never derived from lookup data or the local title.
export function addSonarrSeries(
  config: ArrConfig,
  tmdbId: number,
  fetchImpl?: FetchImplementation,
): Effect.Effect<WatchlistAddOutcome> {
  if (!isConfigured(config)) return Effect.succeed({ result: "unavailable" });
  return Effect.gen(function* () {
    const existing = yield* fetchSonarrSeries(config, fetchImpl);
    if (existing.status !== "ok") return { result: "unavailable" };
    const trackedByTmdb = existing.value.find(
      (series) => series.externalIds?.tmdb === tmdbId,
    );
    if (trackedByTmdb) {
      return { result: "already_exists", titleSlug: trackedByTmdb.titleSlug };
    }

    const lookup = yield* requestJson(
      config,
      `series/lookup?term=${encodeURIComponent(`tmdb:${tmdbId}`)}`,
      SonarrSeriesListSchema,
      {},
      fetchImpl,
    );
    if (lookup.status === "unavailable") return { result: "unavailable" };
    if (lookup.status !== "ok") return { result: "error" };
    if (lookup.value.length === 0) {
      return { result: "not_found" };
    }
    const series = lookup.value[0];
    if (!series.title || series.tvdbId === undefined) {
      return { result: "not_found" };
    }
    const trackedByTvdb = existing.value.find(
      (item) => item.externalIds?.tvdb === series.tvdbId,
    );
    if (trackedByTvdb) {
      return { result: "already_exists", titleSlug: trackedByTvdb.titleSlug };
    }

    const added = yield* requestJson(
      config,
      "series",
      SonarrSeriesSchema,
      postJson({
        title: series.title,
        titleSlug: series.titleSlug,
        year: series.year,
        tvdbId: series.tvdbId,
        tmdbId: series.tmdbId,
        imdbId: series.imdbId,
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

    const verified = yield* fetchSonarrSeries(config, fetchImpl);
    if (verified.status !== "ok") return { result: "unavailable" };
    const written = verified.value.find(
      (item) => item.externalIds?.tvdb === series.tvdbId,
    );
    return written
      ? { result: "added", titleSlug: written.titleSlug }
      : { result: "error" };
  });
}
