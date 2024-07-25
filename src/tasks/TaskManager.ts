import PQueue from "p-queue";
import Logger from "../utils/Logger.js";
import config from "../utils/config.js";
import LiveCheckTask from "./LiveCheckTask.js";
import type { Task } from "./types.js";

export default class TaskManager {
	private queue: PQueue;
	private tasks: Task[];
	private logger = new Logger("TaskManager");

	constructor() {
		this.queue = new PQueue({ concurrency: 1 });
		this.tasks = [new LiveCheckTask(config.YT_CHANNEL_NAMES)];
	}

	public async runTasks(): Promise<void> {
		for (const task of this.tasks) {
			this.queue.add(async () => {
				try {
					this.logger.debug(`Running task: ${task.name}`);
					await task.run();
				} catch (err) {
					this.logger.error(`Error running task: ${task.name}`, err);
				}
			});
		}
	}
}
