import PQueue from "p-queue";

import { debug, error } from "../utils/logging.js";
import LiveCheckTask from "./LiveCheckTask.js";
import type { Task } from "./types.js";
import config from "../utils/config.js";

export default class TaskManager {
	private queue: PQueue;
	private tasks: Task[];

	constructor() {
		this.queue = new PQueue({ concurrency: 1 });
		this.tasks = [new LiveCheckTask(config.YT_CHANNEL_NAMES)];
	}

	public async runTasks(): Promise<void> {
		for (const task of this.tasks) {
			this.queue.add(async () => {
				try {
					debug(`Running task: ${task.name}`);
					await task.run();
				} catch (err) {
					error(`Error running task: ${task.name}`, err);
				}
			});
		}
	}
}
