import type Logger from "../utils/Logger.js";
import { sendNotification } from "../utils/notifications.js";
import { checkYouTubeLiveStatus, getYouTubeLiveUrl } from "../utils/youtube.js";
import { Task } from "./types.js";

export default class LiveCheckTask extends Task {
	public name = "YT Live Check";

	private logger: Logger;
	private previousStatuses = new Map<string, boolean>();
	private numPreviousFailures = 0;

	public constructor(
		private channelNames: string[],
		parentLogger: Logger,
	) {
		super();
		this.logger = parentLogger.extend("LiveCheckTask");
	}

	public async run() {
		const results = await Promise.allSettled(
			this.channelNames.map(async (username) => {
				const result = await checkYouTubeLiveStatus({ username });
				this.logger.debug(`${username} is ${result.isLive ? "" : "NOT "}live`);

				const isLivePrevious = this.previousStatuses.get(username) ?? false;
				if (result.isLive && !isLivePrevious) {
					this.logger.info(`${username} is live: sending notification`);
					await sendNotification({
						title: `${username} is LIVE on YouTube!`,
						message: result.title,
						url: getYouTubeLiveUrl(username),
					});
				} else if (!result.isLive && isLivePrevious) {
					this.logger.info(`${username} is no longer live on YouTube`);
				}

				this.previousStatuses.set(username, result.isLive);
			}),
		);

		const failures = results.filter((r) => r.status === "rejected");
		this.handleFailures(failures);
	}

	private handleFailures(failures: PromiseRejectedResult[]) {
		if (failures.length > 0) {
			// Don't sent a notification if there's an occasional error
			const loggerMethod = this.numPreviousFailures >= 2 ? "error" : "warn";
			for (const result of failures) {
				this.logger[loggerMethod](
					"Failed to check live status:",
					result.reason,
				);
			}
			this.numPreviousFailures += 1;
		} else {
			this.numPreviousFailures = 0;
		}
	}
}
