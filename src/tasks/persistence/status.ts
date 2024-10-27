import { getDoc, upsertDoc } from "@micthiesen/mitools/docstore";
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

const logger = new Logger("Persistence.Status");

export function getChannelStatus(username: string, platform: Platform): ChannelStatus {
	const pk = statusPk(username);
	const status = getDoc<ChannelStatus>(pk);

	if (status) {
		logger.debug(`Found status for ${pk} in DB`, status);
		return status;
	}

	logger.debug(`No status found in DB for ${pk}; returning default`);
	return { username, platform, isLive: false };
}

export function upsertChannelStatus(status: ChannelStatus): void {
	const pk = statusPk(status.username);
	upsertDoc<ChannelStatus>(pk, status);
	logger.debug(`Upserted status for ${pk} in DB`, status);
}

function statusPk(username: string) {
	return `$channel-status#${username}`;
}
