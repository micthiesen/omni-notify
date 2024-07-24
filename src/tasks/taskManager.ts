import PQueue from "p-queue";
import { Task } from "./task";

// Import your tasks
import { task1 } from "./task1";
import { task2 } from "./task2";

export class TaskManager {
  private queue: PQueue;
  private tasks: Task[];

  constructor() {
    this.queue = new PQueue({ concurrency: 1 });
    this.tasks = [task1, task2];
  }

  public async runTasks(): Promise<void> {
    for (const task of this.tasks) {
      this.queue.add(async () => {
        console.log(`Running task: ${task.name}`);
        await task.run();
      });
    }
  }
}
