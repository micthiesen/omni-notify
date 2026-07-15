import config from "../utils/config.js";
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

export async function fetchWatchHistory(): Promise<FetchResult<WatchedItem[]>> {
  try {
    return { status: "ok", value: await client().fetchWatchHistory() };
  } catch (error) {
    return unavailable(error);
  }
}

export async function fetchInProgress(): Promise<FetchResult<InProgressItem[]>> {
  try {
    return { status: "ok", value: await client().fetchInProgress() };
  } catch (error) {
    return unavailable(error);
  }
}

export async function fetchLibraryIndex(): Promise<FetchResult<MediaItem[]>> {
  try {
    return { status: "ok", value: await client().fetchLibraryIndex() };
  } catch (error) {
    return unavailable(error);
  }
}
