import PQueue from "p-queue";
import type { Task } from "./task.js";

// Import your tasks
import { task1 } from "./task1.js";
import { task2 } from "./task2.js";
import type { Config } from "../config.js";

export class TaskManager {
	private queue: PQueue;
	private tasks: Task[];

	constructor(private config: Config) {
		this.queue = new PQueue({ concurrency: 1 });
		this.tasks = [task1, task2];
	}

	public async runTasks(): Promise<void> {
		for (const task of this.tasks) {
			this.queue.add(async () => {
				console.log(`Running task: ${task.name}`);
				await task.run(this.config);
			});
		}
	}
}
