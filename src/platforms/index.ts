import { fetchKickLiveStatus, getKickLiveUrl } from "./kick.js";
import { fetchYouTubeLiveStatus, getYouTubeLiveUrl } from "./youtube.js";

export type FetchedStatusLive = {
	isLive: true;
	title: string;
	viewerCount?: number;
	debugContext?: Record<string, unknown>;
};
export type FetchedStatusOffline = {
	isLive: false;
	debugContext?: Record<string, unknown>;
};
export type FetchedStatus = FetchedStatusLive | FetchedStatusOffline;

export enum Platform {
	YouTube = "youtube",
	Kick = "kick",
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
	[Platform.Kick]: {
		platform: Platform.Kick,
		displayName: "Kick",
		getLiveUrl: getKickLiveUrl,
		fetchLiveStatus: fetchKickLiveStatus,
	},
};
