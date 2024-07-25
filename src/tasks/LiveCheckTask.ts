import config from "../utils/config.js";
import { debug, error, info } from "../utils/logging.js";
import { sendNotification } from "../utils/notifications.js";
import { checkYouTubeLiveStatus, getYouTubeLiveUrl } from "../utils/youtube.js";
import { Task } from "./types.js";

export default class LiveCheckTask extends Task {
	public name = "YT Live Check";

	private previousStatuses = new Map<string, boolean>();

	public async run() {
		const results = await Promise.allSettled(
			config.YT_CHANNEL_NAMES.map(async (username) => {
				const isLive = await checkYouTubeLiveStatus({ username });
				debug(`${username} is ${isLive ? "" : "NOT "}live`);

				const isLivePrevious = this.previousStatuses.get(username) ?? false;
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

				this.previousStatuses.set(username, isLive);
			}),
		);

		const failed = results.filter((r) => r.status === "rejected");
		for (const result of failed) {
			error("Failed to check live status:", result.reason);
		}
	}
}
