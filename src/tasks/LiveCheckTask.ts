import { debug, error, info } from "../utils/logging.js";
import { sendNotification } from "../utils/notifications.js";
import { checkYouTubeLiveStatus, getYouTubeLiveUrl } from "../utils/youtube.js";
import { Task } from "./types.js";

export default class LiveCheckTask extends Task {
	public name = "YT Live Check";

	private previousStatuses = new Map<string, boolean>();

	public constructor(private channelNames: string[]) {
		super();
	}

	public async run() {
		const results = await Promise.allSettled(
			this.channelNames.map(async (username) => {
				const result = await checkYouTubeLiveStatus({ username });
				debug(`${username} is ${result.isLive ? "" : "NOT "}live`);

				const isLivePrevious = this.previousStatuses.get(username) ?? false;
				if (result.isLive && !isLivePrevious) {
					info(`${username} is live: sending notification`);
					await sendNotification({
						title: `${username} is LIVE on YouTube!`,
						message: result.title,
						url: getYouTubeLiveUrl(username),
					});
				} else if (!result.isLive && isLivePrevious) {
					info(`${username} is no longer live on YouTube`);
				}

				this.previousStatuses.set(username, result.isLive);
			}),
		);

		const failed = results.filter((r) => r.status === "rejected");
		for (const result of failed) {
			error("Failed to check live status:", result.reason);
		}
	}
}
