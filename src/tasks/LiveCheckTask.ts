import { formatDistance, formatDistanceToNow } from "date-fns";
import BetterMap from "../utils/BetterMap.js";
import type Logger from "../utils/Logger.js";
import { sendNotification } from "../utils/notifications.js";
import {
	type LiveStatusLive,
	type LiveStatusOffline,
	checkYouTubeLiveStatus,
	getYouTubeLiveUrl,
} from "../utils/youtube.js";
import { Task } from "./types.js";

type ChannelStatusLive = {
	isLive: true;
	title: string;
	startedAt: Date;
};
type ChannelStatusOffline =
	| {
			isLive: false;
			lastEndedAt?: undefined;
			lastStartedAt?: undefined;
	  }
	| {
			isLive: false;
			lastEndedAt: Date;
			lastStartedAt: Date;
	  };
type ChannelStatus = ChannelStatusLive | ChannelStatusOffline;

export default class LiveCheckTask extends Task {
	public name = "YT Live Check";

	private logger: Logger;
	private statuses: BetterMap<string, ChannelStatus>;
	private numPreviousFailures = 0;

	public constructor(
		private channelNames: string[],
		parentLogger: Logger,
	) {
		super();
		this.logger = parentLogger.extend("LiveCheckTask");
		this.statuses = new BetterMap(channelNames.map((n) => [n, { isLive: false }]));
	}

	public async run() {
		const results = await Promise.allSettled(
			this.channelNames.map(async (username) => {
				const currentStatus = await checkYouTubeLiveStatus({ username });
				const previousStatus = this.statuses.getOrThrow(username);
				this.logger.debug(`${username} is ${currentStatus.isLive ? "" : "NOT "}live`);

				if (currentStatus.isLive && !previousStatus.isLive) {
					await this.handleLiveEvent(username, currentStatus, previousStatus);
				} else if (!currentStatus.isLive && previousStatus.isLive) {
					await this.handleOfflineEvent(username, currentStatus, previousStatus);
				}
			}),
		);

		const failures = results.filter((r) => r.status === "rejected");
		this.handleFailures(failures);
	}

	private async handleLiveEvent(
		username: string,
		{ title }: LiveStatusLive,
		{ lastEndedAt, lastStartedAt }: ChannelStatusOffline,
	) {
		this.logger.info(`${username} is live: sending notification`);

		const lastLiveMessage = (() => {
			if (!lastEndedAt) return null;
			const ago = formatDistanceToNow(lastEndedAt);
			const duration = formatDistance(lastEndedAt, lastStartedAt);
			return `Last live ${ago} ago for ${duration}`;
		})();
		const message = (() => {
			if (!lastLiveMessage) return title;
			return `${title}\n\n${lastLiveMessage}`;
		})();

		await sendNotification({
			title: `${username} is LIVE on YouTube!`,
			message,
			url: getYouTubeLiveUrl(username),
			url_title: "Watch on YouTube",
		});

		this.statuses.set(username, { isLive: true, startedAt: new Date(), title });
	}

	private async handleOfflineEvent(
		username: string,
		_: LiveStatusOffline,
		{ startedAt }: ChannelStatusLive,
	) {
		const lastEndedAt = new Date();
		this.logger.info(`${username} is no longer live: sending notification`);

		const duration = formatDistance(lastEndedAt, startedAt);
		await sendNotification({
			title: `${username} is now offline`,
			message: `Streamed for ${duration}`,
		});

		this.statuses.set(username, {
			isLive: false,
			lastEndedAt,
			lastStartedAt: startedAt,
		});
	}

	private handleFailures(failures: PromiseRejectedResult[]) {
		if (failures.length > 0) {
			// Don't sent a notification if there's an occasional error
			const loggerMethod = this.numPreviousFailures >= 2 ? "error" : "warn";
			for (const result of failures) {
				this.logger[loggerMethod]("Failed to check live status:", result.reason);
			}
			this.numPreviousFailures += 1;
		} else {
			this.numPreviousFailures = 0;
		}
	}
}
