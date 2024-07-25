import { sendNotification } from "../notifications.js";
import { checkYouTubeLiveStatus, getYouTubeLiveUrl } from "../youtube.js";
import type { Task } from "./task.js";

const PREVIOUS_STATUSES = new Map<string, boolean>();

export const task1: Task = {
	name: "YT Live Check",
	run: async (config) => {
		const results = await Promise.allSettled(
			config.YT_CHANNEL_NAMES.map(async (username) => {
				const isLive = await checkYouTubeLiveStatus({ username });
				console.log(`${username} is ${isLive ? "" : "NOT "} live`);

				const isLivePrevious = PREVIOUS_STATUSES.get(username) ?? false;
				if (isLive && !isLivePrevious) {
					await sendNotification({
						title: "Live on YouTube",
						message: `${username} is LIVE on YouTube!`,
						url: getYouTubeLiveUrl(username),
					});
				} else if (!isLive && isLivePrevious) {
					console.log(`${username} is no longer live on YouTube`);
				}

				PREVIOUS_STATUSES.set(username, isLive);
			}),
		);

		const failed = results.filter((r) => r.status === "rejected");
		for (const result of failed) {
			console.log("Failed to check live status:", result.reason);
			await sendNotification({
				title: "Live Check Failure",
				message: `${result.reason}`,
			});
		}
	},
};
