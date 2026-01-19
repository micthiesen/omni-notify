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
};

export type FetchedStatusOffline = {
	status: LiveStatus.Offline;
};

export type FetchedStatusUnknown = {
	status: LiveStatus.Unknown;
	error: string;
};

export type FetchedStatus = FetchedStatusLive | FetchedStatusOffline | FetchedStatusUnknown;

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
