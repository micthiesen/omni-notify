import { fetchTwitchLiveStatus, getTwitchLiveUrl } from "./twitch.js";
import { fetchYouTubeLiveStatus, getYouTubeLiveUrl } from "./youtube.js";

export enum LiveStatus {
  Live = "live",
  Offline = "offline",
  Unknown = "unknown",
}

export type FetchedStatusLive = {
  status: LiveStatus.Live;
  title: string;
  viewerCount?: number;
  category?: string;
};

export type FetchedStatusOffline = {
  status: LiveStatus.Offline;
};

export type FetchedStatusUnknown = {
  status: LiveStatus.Unknown;
  error: string;
};

export type FetchedStatus =
  | FetchedStatusLive
  | FetchedStatusOffline
  | FetchedStatusUnknown;

export enum Platform {
  YouTube = "youtube",
  Twitch = "twitch",
}

export interface PlatformConfig {
  platform: Platform;
  displayName: string;
  getLiveUrl: (username: string) => string;
  fetchLiveStatus: (args: { username: string }) => Promise<FetchedStatus>;
}

export const platformConfigs: Record<Platform, PlatformConfig> = {
  [Platform.YouTube]: {
    platform: Platform.YouTube,
    displayName: "YouTube",
    getLiveUrl: getYouTubeLiveUrl,
    fetchLiveStatus: fetchYouTubeLiveStatus,
  },
  [Platform.Twitch]: {
    platform: Platform.Twitch,
    displayName: "Twitch",
    getLiveUrl: getTwitchLiveUrl,
    fetchLiveStatus: fetchTwitchLiveStatus,
  },
};

/** Returns url and url_title fields for notifications */
export function getNotificationUrlFields(platform: Platform, username: string) {
  const config = platformConfigs[platform];
  return {
    url: config.getLiveUrl(username),
    url_title: `Watch on ${config.displayName}`,
  };
}
