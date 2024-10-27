import { type Logger, notify } from "@micthiesen/mitools";
import { BetterMap } from "@micthiesen/mitools/dist/collections/maps.js";
import { formatDistance, formatDistanceToNow } from "date-fns";
import config from "../utils/config.js";
import {
	type FetchedStatus,
	type FetchedStatusLive,
	type FetchedStatusOffline,
	fetchYouTubeLiveStatus,
	getYouTubeLiveUrl,
} from "../utils/youtube.js";
import { Task } from "./types.js";

type ChannelStatusLive = {
	isLive: true;
	title: string;
	startedAt: Date;
	maxViewerCount?: number;
};
type ChannelStatusOffline =
	| {
			isLive: false;
			lastEndedAt?: undefined;
			lastStartedAt?: undefined;
			lastViewerCount?: undefined;
	  }
	| {
			isLive: false;
			lastEndedAt: Date;
			lastStartedAt: Date;
			lastViewerCount?: number;
	  };
type ChannelStatus = ChannelStatusLive | ChannelStatusOffline;

export default class LiveCheckTask extends Task {
	public name = "Live Check";

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
				const fetchedStatus = await fetchYouTubeLiveStatus({ username });
				const previousStatus = this.statuses.getOrThrow(username);
				this.logger.debug(`${username} is ${fetchedStatus.isLive ? "" : "NOT "}live`);

				if (fetchedStatus.isLive && !previousStatus.isLive) {
					await this.handleLiveEvent(username, fetchedStatus, previousStatus);
				} else if (!fetchedStatus.isLive && previousStatus.isLive) {
					await this.handleOfflineEvent(username, fetchedStatus, previousStatus);
				}

				this.handleMaxViewerCount(username, fetchedStatus);
			}),
		);

		const failures = results.filter((r) => r.status === "rejected");
		this.handleFailures(failures);
	}

	private async handleLiveEvent(
		username: string,
		{ title }: FetchedStatusLive,
		{ lastEndedAt, lastStartedAt, lastViewerCount }: ChannelStatusOffline,
	) {
		this.logger.info(`${username} is live`);

		const lastLiveMessage = (() => {
			if (!lastEndedAt) return null;
			const ago = formatDistanceToNow(lastEndedAt);
			const duration = formatDistance(lastEndedAt, lastStartedAt);
			const text = `Last live ${ago} ago for ${duration}`;
			return lastViewerCount ? `${text} with ${formatCount(lastViewerCount)}` : text;
		})();
		const message = (() => {
			if (!lastLiveMessage) return title;
			return `${title}\n\n${lastLiveMessage}`;
		})();

		await notify({
			title: `${username} is LIVE on YouTube!`,
			message,
			url: getYouTubeLiveUrl(username),
			url_title: "Watch on YouTube",
		});

		this.statuses.set(username, { isLive: true, startedAt: new Date(), title });
	}

	private async handleOfflineEvent(
		username: string,
		_: FetchedStatusOffline,
		{ startedAt, maxViewerCount }: ChannelStatusLive,
	) {
		const lastEndedAt = new Date();
		this.logger.info(`${username} is no longer live`);

		if (config.OFFLINE_NOTIFICATIONS) {
			const duration = formatDistance(lastEndedAt, startedAt);
			const durationText = `Streamed for ${duration}`;
			const message = maxViewerCount
				? `${durationText} with ${formatCount(maxViewerCount)}`
				: durationText;

			await notify({ title: `${username} is now offline`, message });
		}

		this.statuses.set(username, {
			isLive: false,
			lastEndedAt,
			lastStartedAt: startedAt,
			lastViewerCount: maxViewerCount,
		});
	}

	private handleMaxViewerCount(username: string, fetchedStatus: FetchedStatus) {
		if (!fetchedStatus.isLive || fetchedStatus.viewerCount === undefined) return;

		const updatedStatus = this.statuses.getOrThrow(username);
		if (!updatedStatus.isLive) return;

		if (
			updatedStatus.maxViewerCount === undefined ||
			fetchedStatus.viewerCount > updatedStatus.maxViewerCount
		) {
			updatedStatus.maxViewerCount = fetchedStatus.viewerCount;
		}
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

function formatCount(count: number) {
	return `${count.toLocaleString()} viewers`;
}
