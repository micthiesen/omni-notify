import config from "../utils/config.js";
import type { ArrConfig } from "./arr/client.js";
import { addRadarrMovie, fetchRadarrMovies } from "./arr/radarr.js";
import { addSonarrSeries, fetchSonarrSeries } from "./arr/sonarr.js";
import type {
  AddToWatchlistResult,
  ExternalIds,
  FetchResult,
  MediaItem,
} from "./types.js";
import { MediaType } from "./types.js";

/**
 * Account/service-scoped watchlist client, split from the media library
 * bridge because watchlists typically live on a different surface than the
 * library itself (an account-level cloud API, or a separate service
 * entirely), with its own base URL and auth.
 *
 * Radarr owns movies and Sonarr owns TV series. Treat either service being
 * unavailable as an unavailable combined watchlist so callers never mistake a
 * partial response for the complete tracked state.
 */

export interface WatchlistAddRequest {
  tmdbId: number;
  mediaType: MediaType;
  title: string;
  year?: number;
  externalIds?: ExternalIds;
}

export async function fetchWatchlist(): Promise<FetchResult<MediaItem[]>> {
  const [movies, series] = await Promise.all([
    fetchRadarrMovies(radarrConfig()),
    fetchSonarrSeries(sonarrConfig()),
  ]);
  if (movies.status !== "ok" || series.status !== "ok") {
    return { status: "unavailable", reason: "Radarr or Sonarr is unavailable" };
  }
  return { status: "ok", value: [...movies.value, ...series.value] };
}

export async function addToWatchlist(
  request: WatchlistAddRequest,
): Promise<AddToWatchlistResult> {
  return request.mediaType === MediaType.Movie
    ? addRadarrMovie(radarrConfig(), request.tmdbId)
    : addSonarrSeries(sonarrConfig(), request.tmdbId);
}

function radarrConfig(): ArrConfig {
  return {
    url: config.RADARR_URL,
    apiKey: config.RADARR_API_KEY,
    rootFolderPath: config.RADARR_ROOT_FOLDER_PATH,
    qualityProfileId: config.RADARR_QUALITY_PROFILE_ID,
  };
}

function sonarrConfig(): ArrConfig {
  return {
    url: config.SONARR_URL,
    apiKey: config.SONARR_API_KEY,
    rootFolderPath: config.SONARR_ROOT_FOLDER_PATH,
    qualityProfileId: config.SONARR_QUALITY_PROFILE_ID,
  };
}
