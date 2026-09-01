import config from "../utils/config.js";
import { Effect } from "effect";
import { runPromise } from "../effect/interop.js";
import { createPlexClient } from "./plex/client.js";
import type { FetchResult, InProgressItem, MediaItem, WatchedItem } from "./types.js";

/**
 * Plex-backed view of the local media library. An unavailable Plex instance is
 * deliberately different from an empty library: callers must not make
 * recommendation decisions from missing state.
 */

function client() {
  return createPlexClient(config.PLEX_URL, config.PLEX_TOKEN, config.PLEX_ACCOUNT_ID);
}

function unavailable(error: unknown): FetchResult<never> {
  return {
    status: "unavailable",
    reason: error instanceof Error ? error.message : String(error),
  };
}

export function fetchWatchHistoryEffect(): Effect.Effect<FetchResult<WatchedItem[]>> {
  return Effect.suspend(() => client().fetchWatchHistory()).pipe(
    Effect.map((value) => ({ status: "ok" as const, value })),
    Effect.catchAll((error) => Effect.succeed(unavailable(error))),
  );
}

export function fetchInProgressEffect(): Effect.Effect<FetchResult<InProgressItem[]>> {
  return Effect.suspend(() => client().fetchInProgress()).pipe(
    Effect.map((value) => ({ status: "ok" as const, value })),
    Effect.catchAll((error) => Effect.succeed(unavailable(error))),
  );
}

export function fetchLibraryIndexEffect(): Effect.Effect<FetchResult<MediaItem[]>> {
  return Effect.suspend(() => client().fetchLibraryIndex()).pipe(
    Effect.map((value) => ({ status: "ok" as const, value })),
    Effect.catchAll((error) => Effect.succeed(unavailable(error))),
  );
}

export const fetchWatchHistory = () => runPromise(fetchWatchHistoryEffect());
export const fetchInProgress = () => runPromise(fetchInProgressEffect());
export const fetchLibraryIndex = () => runPromise(fetchLibraryIndexEffect());
