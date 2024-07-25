import PQueue from "p-queue";

import { task1 } from "./task1.js";
import type { Config } from "../config.js";

export class TaskManager {
	private queue: PQueue;
	private tasks: Task[];

	constructor(private config: Config) {
		this.queue = new PQueue({ concurrency: 1 });
		this.tasks = [task1];
	}

	public async runTasks(): Promise<void> {
		for (const task of this.tasks) {
			this.queue.add(async () => {
				try {
					console.log(`Running task: ${task.name}`);
					await task.run(this.config);
				} catch (error) {
					console.error(`Error running task: ${task.name}`, error);
				}
			});
		}
	}
}

export interface Task {
	name: string;
	run: (config: Config) => Promise<void>;
}
