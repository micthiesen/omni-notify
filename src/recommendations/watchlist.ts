import type {
  AddToWatchlistResult,
  ExternalIds,
  FetchResult,
  MediaItem,
  MediaType,
} from "./types.js";

/**
 * Account/service-scoped watchlist client, split from the media library
 * bridge because watchlists typically live on a different surface than the
 * library itself (an account-level cloud API, or a separate service
 * entirely), with its own base URL and auth.
 *
 * Config: WATCHLIST_SERVICE_URL / WATCHLIST_SERVICE_TOKEN.
 */

export interface WatchlistAddRequest {
  tmdbId: number;
  mediaType: MediaType;
  title: string;
  year?: number;
  externalIds?: ExternalIds;
}

export async function fetchWatchlist(): Promise<FetchResult<MediaItem[]>> {
  // TODO: implement against the chosen watchlist backend.
  // Used by outcome sync (was a recommendation removed unwatched?) and by
  // hard filters (never recommend something already on the watchlist).
  return { status: "unavailable", reason: "watchlist service not implemented" };
}

export async function addToWatchlist(
  _request: WatchlistAddRequest,
): Promise<AddToWatchlistResult> {
  // TODO: implement against the chosen watchlist backend. Must report
  // already_exists distinctly (the pipeline promotes the backup candidate),
  // and should verify the write landed before returning "added".
  return "unavailable";
}
