import type { Logger } from "@micthiesen/mitools/logging";
import cron from "node-cron";
import PQueue from "p-queue";
import type { ScheduledTask } from "./ScheduledTask.js";

type RegisteredTask = {
  task: ScheduledTask;
  queue: PQueue;
};

type StartedTask = RegisteredTask & {
  cronJob: cron.ScheduledTask;
};

export class Scheduler {
  private registeredTasks: RegisteredTask[] = [];
  private startedTasks: StartedTask[] = [];
  private logger: Logger;

  constructor(parentLogger: Logger) {
    this.logger = parentLogger.extend("Scheduler");
  }

  /** Register a task to be scheduled. Call before start(). */
  public register(task: ScheduledTask): void {
    if (!cron.validate(task.schedule)) {
      throw new Error(
        `Invalid cron expression "${task.schedule}" for task "${task.name}"`,
      );
    }

    // Each task gets its own queue with concurrency=1 to prevent overlapping runs
    const queue = new PQueue({ concurrency: 1 });

    this.registeredTasks.push({ task, queue });
    this.logger.info(`Registered task "${task.name}" with schedule "${task.schedule}"`);
  }

  /** Start all registered cron jobs and execute each task immediately. */
  public start(): void {
    for (const { task, queue } of this.registeredTasks) {
      // Create cron job (auto-starts in node-cron v4)
      const cronJob = cron.schedule(task.schedule, () => {
        queue.add(() => this.executeTask(task));
      });

      this.startedTasks.push({ task, queue, cronJob });

      if (task.runOnStartup) {
        queue.add(() => this.executeTask(task));
      }
    }
    this.logger.info(`Started ${this.startedTasks.length} scheduled task(s)`);
  }

  /** Stop all cron jobs and wait for pending tasks to complete. */
  public async shutdown(): Promise<void> {
    // Stop all cron jobs from scheduling new runs
    for (const { cronJob } of this.startedTasks) {
      cronJob.stop();
    }

    // Wait for all queues to drain
    const pendingCounts = this.startedTasks.map(
      ({ queue }) => queue.size + queue.pending,
    );
    const totalPending = pendingCounts.reduce((a, b) => a + b, 0);

    if (totalPending > 0) {
      this.logger.info(`Waiting for ${totalPending} pending task(s) to complete...`);
      await Promise.all(this.startedTasks.map(({ queue }) => queue.onIdle()));
    }
  }

  private async executeTask(task: ScheduledTask): Promise<void> {
    try {
      if (task.jitterMs > 0) {
        await randomSleep(task.jitterMs);
      }

      this.logger.debug(`Running task: ${task.name}`);
      await task.run();
    } catch (err) {
      this.logger.error(`Error running task "${task.name}"`, err);
    }
  }
}

function randomSleep(maxMs: number): Promise<void> {
  const delay = Math.floor(Math.random() * maxMs);
  return new Promise((resolve) => setTimeout(resolve, delay));
}
