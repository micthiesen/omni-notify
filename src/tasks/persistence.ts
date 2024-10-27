import { getDoc, upsertDoc } from "@micthiesen/mitools/docstore";
import { Logger } from "@micthiesen/mitools/logging";

export type ChannelStatusLive = {
	username: string;
	isLive: true;
	title: string;
	startedAt: Date;
	maxViewerCount?: number;
};
export type ChannelStatusOffline =
	| {
			username: string;
			isLive: false;
			lastEndedAt?: undefined;
			lastStartedAt?: undefined;
			lastViewerCount?: undefined;
	  }
	| {
			username: string;
			isLive: false;
			lastEndedAt: Date;
			lastStartedAt: Date;
			lastViewerCount?: number;
	  };
export type ChannelStatus = ChannelStatusLive | ChannelStatusOffline;

const logger = new Logger("persistence");

export function getChannelStatus(username: string): ChannelStatus {
	const pk = statusPk(username);
	const status = getDoc<ChannelStatus>(pk);

	if (status) {
		logger.debug(`Found status for ${pk} in DB`, status);
		return status;
	}

	logger.debug(`No status found in DB for ${pk}; returning default`);
	return { username, isLive: false };
}

export function upsertChannelStatus(status: ChannelStatus): void {
	const pk = statusPk(status.username);
	upsertDoc<ChannelStatus>(pk, status);
	logger.debug(`Upserted status for ${pk} in DB`, status);
}

function statusPk(username: string) {
	return `$channel-status#${username}`;
}
