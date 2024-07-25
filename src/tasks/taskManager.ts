import PQueue from "p-queue";

import type { Config } from "../config.js";
import { debug, error } from "../logging.js";
import { task1 } from "./task1.js";

export class TaskManager {
	private queue: PQueue;
	private tasks: Task[];

	constructor() {
		this.queue = new PQueue({ concurrency: 1 });
		this.tasks = [task1];
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

export interface Task {
	name: string;
	run: () => Promise<void>;
}
