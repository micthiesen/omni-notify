import config from "../config.js";
import { debug, error, info } from "../logging.js";
import { sendNotification } from "../notifications.js";
import { checkYouTubeLiveStatus, getYouTubeLiveUrl } from "../youtube.js";
import type { Task } from "./taskManager.js";

const PREVIOUS_STATUSES = new Map<string, boolean>();

export const task1: Task = {
	name: "YT Live Check",
	run: async () => {
		const results = await Promise.allSettled(
			config.YT_CHANNEL_NAMES.map(async (username) => {
				const isLive = await checkYouTubeLiveStatus({ username });
				debug(`${username} is ${isLive ? "" : "NOT "}live`);

				const isLivePrevious = PREVIOUS_STATUSES.get(username) ?? false;
				if (isLive && !isLivePrevious) {
					info(`${username} is live; sending notification`);
					await sendNotification({
						title: "LIVE on YouTube",
						message: `${username} is LIVE on YouTube!`,
						url: getYouTubeLiveUrl(username),
					});
				} else if (!isLive && isLivePrevious) {
					info(`${username} is no longer live on YouTube`);
				}

				PREVIOUS_STATUSES.set(username, isLive);
			}),
		);

		const failed = results.filter((r) => r.status === "rejected");
		for (const result of failed) {
			error("Failed to check live status:", result.reason);
		}
	},
};
