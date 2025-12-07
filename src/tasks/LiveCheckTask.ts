import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { formatDistance, formatDistanceToNow } from "date-fns";
import {
	type FetchedStatus,
	type FetchedStatusLive,
	type FetchedStatusOffline,
	type Platform,
	type PlatformConfig,
	platformConfigs,
} from "../platforms/index.js";
import appConfig from "../utils/config.js";
import {
	type ChannelMetrics,
	getChannelMetrics,
	upsertChannelMetrics,
} from "./persistence/metrics.js";
import {
	type ChannelStatusLive,
	type ChannelStatusOffline,
	getChannelStatus,
	upsertChannelStatus,
} from "./persistence/status.js";
import { Task } from "./types.js";

export default class LiveCheckTask extends Task {
	public name = "Live Check";
	private channels: { username: string; config: PlatformConfig }[] = [];

	private logger: Logger;
	private numPreviousFailures = 0;
	private runNumber = 1;

	public constructor(channels: [Platform, string[]][], parentLogger: Logger) {
		super();
		for (const [platform, usernames] of channels) {
			const config = platformConfigs[platform];
			for (const username of usernames) {
				this.channels.push({ username, config });
			}
		}

		this.logger = parentLogger.extend("LiveCheckTask");
	}

	public async run() {
		const handleMetrics = this.runNumber === 9;
		if (handleMetrics) this.runNumber = 0;

		const results = await Promise.allSettled(
			this.channels.map(async ({ username, config }) => {
				const fetchedStatus = await config.fetchLiveStatus({ username });
				const previousStatus = getChannelStatus(username, config.platform);
				const previousMetrics = getChannelMetrics(username);
				this.logger.debug(
					`${username} is ${fetchedStatus.isLive ? "" : "NOT "}live (${this.runNumber})`,
					fetchedStatus,
				);

				if (fetchedStatus.isLive && !previousStatus.isLive) {
					await this.handleLiveEvent(fetchedStatus, previousStatus, config);
				} else if (!fetchedStatus.isLive && previousStatus.isLive) {
					await this.handleOfflineEvent(fetchedStatus, previousStatus, config);
				}

				this.handleMaxViewerCount(username, config.platform, fetchedStatus);

				if (handleMetrics) this.handleChannelMetrics(previousMetrics, fetchedStatus);
			}),
		);

		const failures = results.filter((r) => r.status === "rejected");
		this.handleFailures(failures);
		this.runNumber += 1;
	}

	private async handleLiveEvent(
		{ title, debugContext }: FetchedStatusLive,
		{ username, lastEndedAt, lastStartedAt, lastViewerCount }: ChannelStatusOffline,
		config: PlatformConfig,
	) {
		this.logger.info(`${username} is live`, debugContext);

		const tempExtra = JSON.stringify(debugContext?.metaTag);

		const lastLiveMessage = (() => {
			if (!lastEndedAt) return null;
			const ago = formatDistanceToNow(lastEndedAt);
			const duration = formatDistance(lastEndedAt, lastStartedAt);
			const text = `Last live ${ago} ago for ${duration}`;
			return lastViewerCount ? `${text} with ${formatCount(lastViewerCount)}` : text;
		})();
		const message = (() => {
			if (!lastLiveMessage) return `${title}\n\n${tempExtra}`;
			return `${title}\n\n${lastLiveMessage}\n\n${tempExtra}`;
		})();

		await notify({
			title: `${username} is LIVE on ${config.displayName}!`,
			message,
			url: config.getLiveUrl(username),
			url_title: `Watch on ${config.displayName}`,
		});

		upsertChannelStatus({
			username,
			platform: config.platform,
			isLive: true,
			startedAt: new Date(),
			title,
		});
	}

	private async handleOfflineEvent(
		{ debugContext }: FetchedStatusOffline,
		{ username, startedAt, maxViewerCount }: ChannelStatusLive,
		config: PlatformConfig,
	) {
		const lastEndedAt = new Date();
		this.logger.info(`${username} is no longer live`, debugContext);

		if (appConfig.OFFLINE_NOTIFICATIONS) {
			const duration = formatDistance(lastEndedAt, startedAt);
			const durationText = `Streamed for ${duration}`;
			const message = maxViewerCount
				? `${durationText} with ${formatCount(maxViewerCount)}`
				: durationText;

			await notify({ title: `${username} is now offline`, message });
		}

		upsertChannelStatus({
			username,
			platform: config.platform,
			isLive: false,
			lastEndedAt,
			lastStartedAt: startedAt,
			lastViewerCount: maxViewerCount,
		});
	}

	private handleMaxViewerCount(
		username: string,
		platform: Platform,
		fetchedStatus: FetchedStatus,
	) {
		if (!fetchedStatus.isLive || fetchedStatus.viewerCount === undefined) return;

		const updatedStatus = getChannelStatus(username, platform);
		if (!updatedStatus.isLive) return;

		if (
			updatedStatus.maxViewerCount === undefined ||
			fetchedStatus.viewerCount > updatedStatus.maxViewerCount
		) {
			updatedStatus.maxViewerCount = fetchedStatus.viewerCount;
			upsertChannelStatus(updatedStatus);
			this.logger.info(
				`Updated max viewer count for ${username} to ${fetchedStatus.viewerCount}`,
			);
		}
	}

	private async handleChannelMetrics(
		previousMetrics: ChannelMetrics,
		fetchedStatus: FetchedStatus,
	) {
		if (!fetchedStatus.isLive || fetchedStatus.viewerCount === undefined) return;

		if (fetchedStatus.viewerCount > previousMetrics.maxViewerCount) {
			previousMetrics.maxViewerCount = fetchedStatus.viewerCount;
			upsertChannelMetrics(previousMetrics);
			this.logger.info(
				`Updated all-time max viewer count for ${previousMetrics.username}`,
			);
			await notify({
				title: `New record for ${previousMetrics.username}!`,
				message: `Now at ${formatCount(fetchedStatus.viewerCount)}`,
			});
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
