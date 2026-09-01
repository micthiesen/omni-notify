import type { AddToWatchlistResult, MediaItem } from "../types.js";
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

const RadarrMovieSchema = Schema.Struct({
  id: Schema.optional(Schema.Number),
  title: Schema.optional(Schema.String),
  year: Schema.optional(Schema.Number),
  tmdbId: Schema.optional(Schema.Number),
  imdbId: Schema.optional(Schema.String),
});
const RadarrMoviesSchema = Schema.Array(RadarrMovieSchema);

export function fetchRadarrMovies(
  config: ArrConfig,
  fetchImpl?: FetchImplementation,
): Effect.Effect<{ status: "ok"; value: MediaItem[] } | { status: "unavailable" }> {
  if (!hasArrConnection(config)) return Effect.succeed({ status: "unavailable" });
  return Effect.gen(function* () {
    const response = yield* requestJson(
      config,
      "movie",
      RadarrMoviesSchema,
      {},
      fetchImpl,
    );
    if (response.status !== "ok") {
      return { status: "unavailable" };
    }
    return {
      status: "ok",
      value: response.value.flatMap((movie) => {
        const id = movie.id;
        const title = movie.title;
        const tmdb = movie.tmdbId;
        if (id === undefined || title === undefined || tmdb === undefined) return [];
        return [
          {
            guid: `radarr:${id}`,
            title,
            year: movie.year,
            mediaType: MediaType.Movie,
            externalIds: { tmdb, imdb: movie.imdbId },
          },
        ];
      }),
    };
  });
}

export function addRadarrMovie(
  config: ArrConfig,
  tmdbId: number,
  fetchImpl?: FetchImplementation,
): Effect.Effect<AddToWatchlistResult> {
  if (!isConfigured(config)) return Effect.succeed("unavailable");
  return Effect.gen(function* () {
    const existing = yield* fetchRadarrMovies(config, fetchImpl);
    if (existing.status !== "ok") return "unavailable";
    if (existing.value.some((movie) => movie.externalIds?.tmdb === tmdbId)) {
      return "already_exists";
    }

    const lookup = yield* requestJson(
      config,
      `movie/lookup/tmdb?tmdbId=${encodeURIComponent(tmdbId)}`,
      RadarrMovieSchema,
      {},
      fetchImpl,
    );
    if (lookup.status === "unavailable") return "unavailable";
    if (lookup.status !== "ok") return "error";
    if (!lookup.value.title || lookup.value.tmdbId === undefined) {
      return "not_found";
    }

    const added = yield* requestJson(
      config,
      "movie",
      RadarrMovieSchema,
      postJson({
        title: lookup.value.title,
        year: lookup.value.year,
        tmdbId: lookup.value.tmdbId,
        imdbId: lookup.value.imdbId,
        qualityProfileId: config.qualityProfileId,
        rootFolderPath: config.rootFolderPath,
        monitored: true,
        addOptions: { searchForMovie: true },
      }),
      fetchImpl,
    );
    if (added.status === "unavailable") return "unavailable";
    if (added.status !== "ok") return "error";

    const verified = yield* fetchRadarrMovies(config, fetchImpl);
    if (verified.status !== "ok") return "unavailable";
    return verified.value.some((movie) => movie.externalIds?.tmdb === tmdbId)
      ? "added"
      : "error";
  });
}
