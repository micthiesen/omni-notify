import type { FetchResult, InProgressItem, MediaItem, WatchedItem } from "./types.js";

/**
 * Server-scoped media library bridge: watch history, in-progress state, and
 * the local library index. Deliberately separate from the watchlist client —
 * for most backends these live on different hosts with different auth (e.g.
 * a local server token vs an account-scoped cloud token), and they may end
 * up backed by entirely different services.
 *
 * All methods return three-state FetchResults: "unavailable" must never be
 * treated as an empty library (the pipeline aborts instead of recommending
 * against stale/missing state).
 *
 * Config: MEDIA_SERVER_URL / MEDIA_SERVER_TOKEN.
 */

const NOT_IMPLEMENTED: FetchResult<never> = {
  status: "unavailable",
  reason: "media library bridge not implemented",
};

export async function fetchWatchHistory(): Promise<FetchResult<WatchedItem[]>> {
  // TODO: implement against the chosen media server backend.
  // Return every watched item with viewedAt, viewCount, and (when the backend
  // reports playback progress) completion as a 0-1 fraction of runtime.
  return NOT_IMPLEMENTED;
}

export async function fetchInProgress(): Promise<FetchResult<InProgressItem[]>> {
  // TODO: implement against the chosen media server backend.
  // Return items with partial playback progress (0-1) and lastViewedAt.
  return NOT_IMPLEMENTED;
}

export async function fetchLibraryIndex(): Promise<FetchResult<MediaItem[]>> {
  // TODO: implement against the chosen media server backend.
  // Return all items present in the local library (used as a positive signal
  // for candidates that are already available locally).
  return NOT_IMPLEMENTED;
}
