import { Entity } from "@micthiesen/mitools/entities";
import { Logger } from "@micthiesen/mitools/logging";
import type { Platform } from "../../platforms/index.js";

export type ChannelStatusLive = {
	username: string;
	platform: Platform;
	isLive: true;
	title: string;
	startedAt: Date;
	maxViewerCount?: number;
};
export type ChannelStatusOffline =
	| {
			username: string;
			platform: Platform;
			isLive: false;
			lastEndedAt?: undefined;
			lastStartedAt?: undefined;
			lastViewerCount?: undefined;
	  }
	| {
			username: string;
			platform: Platform;
			isLive: false;
			lastEndedAt: Date;
			lastStartedAt: Date;
			lastViewerCount?: number;
	  };
export type ChannelStatus = ChannelStatusLive | ChannelStatusOffline;

export const ChannelStatusEntity = new Entity<ChannelStatus, ["username"]>(
	"channel-metrics",
	["username"],
);

const logger = new Logger("Persistence.Status");

export function getChannelStatus(username: string, platform: Platform): ChannelStatus {
	const status = ChannelStatusEntity.get({ username });

	if (status) {
		logger.debug(`Found status for ${username} in DB`, status);
		return status;
	}

	logger.debug(`No status found in DB for ${username}; returning default`);
	return { username, platform, isLive: false };
}

export function upsertChannelStatus(status: ChannelStatus): void {
	ChannelStatusEntity.upsert(status);
	logger.debug(`Upserted status for ${status.username} in DB`, status);
}
