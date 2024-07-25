import { checkYouTubeLiveStatus } from "../youtube.js";
import type { Task } from "./task.js";

export const task1: Task = {
	name: "YT Live Check",
	run: async (config) => {
		const results = await Promise.allSettled(
			config.YT_CHANNEL_NAMES.map(async (username) => {
				const check = await checkYouTubeLiveStatus({ username });
				console.log(`${username} is ${check ? "" : "NOT "} live`);
			}),
		);

		const failed = results.filter((r) => r.status === "rejected");
		for (const result of failed) {
			console.log("Failed to check live status:", result.reason);
		}
	},
};
