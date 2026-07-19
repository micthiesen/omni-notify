import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import cron, { type ScheduledTask as CronScheduledTask } from "node-cron";
import PQueue from "p-queue";
import { decideCatchUp } from "./catchUp.js";
import { taskRunBus } from "./events.js";
import {
  finishRunLogCapture,
  runWithLogCapture,
  startRunLogCapture,
} from "./logCapture.js";
import {
  getLastRun,
  getTaskScheduleState,
  makeRunId,
  markInterruptedRuns,
  markScheduleEvaluated,
  recordRunEnd,
  recordRunStart,
  type TaskRunData,
  type TaskRunTrigger,
} from "./persistence.js";

/** Tasks may report a one-line summary of their most recent run. */
interface ProvidesRunSummary {
  getLastRunSummary(): string | undefined;
}

interface HandlesManualRunInput {
  runManual(input: unknown): Promise<void>;
}

/** Tasks may report a friendlier name for the UI; `name` itself stays the load-bearing key. */
interface HasDisplayName {
  displayName?: string;
}

function providesRunSummary(
  task: ScheduledTask,
): task is ScheduledTask & ProvidesRunSummary {
  return typeof (task as Partial<ProvidesRunSummary>).getLastRunSummary === "function";
}

function handlesManualRunInput(
  task: ScheduledTask,
): task is ScheduledTask & HandlesManualRunInput {
  return typeof (task as Partial<HandlesManualRunInput>).runManual === "function";
}

function getDisplayName(task: ScheduledTask): string | undefined {
  return (task as Partial<HasDisplayName>).displayName;
}

export interface TaskInfo {
  name: string;
  displayName?: string;
  schedule: string;
  running: boolean;
  nextRuns: string[];
  lastRun: TaskRunData | null;
}

/**
 * Tracks every registered scheduled task: persists run history (for the UI),
 * exposes next-run times, and supports manual runs serialized on the same
 * per-task queue as scheduled runs so they can never overlap.
 */
export class TaskRegistry {
  private tasks = new Map<
    string,
    { task: ScheduledTask; queue: PQueue; cronTask: CronScheduledTask }
  >();
  private running = new Set<string>();
  private hasRun = new Set<string>();
  private logger: Logger;

  constructor(parentLogger: Logger) {
    this.logger = parentLogger.extend("TaskRegistry");
    const interrupted = markInterruptedRuns();
    if (interrupted > 0) {
      this.logger.warn(`Marked ${interrupted} interrupted task run(s) as errors`);
    }
  }

  /**
   * Wrap a task for the Scheduler. The wrapper funnels scheduled executions
   * through this registry's per-task queue, alongside manual runs.
   */
  public track(task: ScheduledTask): ScheduledTask {
    if (this.tasks.has(task.name)) {
      throw new Error(`Task "${task.name}" is already registered`);
    }
    // Never-started cron instance, used purely to compute upcoming run times.
    const cronTask = cron.createTask(task.schedule, () => {});
    const queue = new PQueue({ concurrency: 1 });
    this.tasks.set(task.name, { task, queue, cronTask });

    const registry = this;
    return new (class extends ScheduledTask {
      public readonly name = task.name;
      public readonly schedule = task.schedule;
      public override readonly jitterMs = task.jitterMs;
      public override readonly runOnStartup = task.runOnStartup;
      public run(): Promise<void> {
        return registry.execute(task.name, "schedule");
      }
    })();
  }

  /** Queue a manual run. Rejects immediately if the task is already running. */
  public runNow(name: string, input?: unknown): { runId: string } {
    const entry = this.tasks.get(name);
    if (!entry) throw new TaskNotFoundError(name);
    if (input !== undefined && !handlesManualRunInput(entry.task)) {
      throw new TaskManualInputUnsupportedError(name);
    }
    if (this.running.has(name) || entry.queue.size + entry.queue.pending > 0) {
      throw new TaskAlreadyRunningError(name);
    }
    const runId = makeRunId(name);
    void this.execute(name, "manual", runId, undefined, input).catch((error) => {
      this.logger.error(`Manual run of "${name}" failed`, error);
    });
    return { runId };
  }

  /** Recover the newest eligible cron occurrence for each infrequent task. */
  public async recoverMissedTasks(now = Date.now()): Promise<void> {
    const recoveries: { name: string; scheduledFor: number }[] = [];

    for (const [name, entry] of this.tasks) {
      const state = getTaskScheduleState(name);
      if (state && state.schedule !== entry.task.schedule) {
        this.logger.info(
          `Schedule changed for "${name}"; starting a new recovery baseline`,
        );
        markScheduleEvaluated(name, entry.task.schedule, now);
        continue;
      }

      const evaluatedThrough = state?.evaluatedThrough ?? getLastRun(name)?.startedAt;
      if (evaluatedThrough === undefined || entry.task.runOnStartup) {
        markScheduleEvaluated(name, entry.task.schedule, now);
        continue;
      }

      const decision = decideCatchUp(entry.task.schedule, evaluatedThrough, now);
      switch (decision.kind) {
        case "run":
          recoveries.push({ name, scheduledFor: decision.scheduledFor });
          break;
        case "stale":
          this.logger.info(
            `Skipping stale missed run of "${name}" from ${new Date(decision.scheduledFor).toISOString()}`,
          );
          markScheduleEvaluated(name, entry.task.schedule, now);
          break;
        case "disabled":
        case "none":
          markScheduleEvaluated(name, entry.task.schedule, now);
          break;
      }
    }

    // Recover sequentially so a reboot cannot unleash several expensive tasks at once.
    for (const recovery of recoveries) {
      this.logger.info(
        `Recovering missed run of "${recovery.name}" from ${new Date(recovery.scheduledFor).toISOString()}`,
      );
      try {
        await this.execute(recovery.name, "catchup", undefined, recovery.scheduledFor);
      } catch (error) {
        this.logger.error(`Catch-up run of "${recovery.name}" failed`, error);
      }
    }
  }

  private async execute(
    name: string,
    trigger: TaskRunTrigger,
    runId?: string,
    scheduledFor?: number,
    manualInput?: unknown,
  ): Promise<void> {
    const entry = this.tasks.get(name);
    if (!entry) throw new TaskNotFoundError(name);

    // The Scheduler fires runOnStartup tasks through the same path as cron
    // runs; the very first scheduled execution of such a task is the boot run.
    if (trigger === "schedule" && entry.task.runOnStartup && !this.hasRun.has(name)) {
      trigger = "startup";
    }
    this.hasRun.add(name);

    await entry.queue.add(async () => {
      // Once an execution starts it counts as this occurrence's attempt, even if
      // it fails or the process exits partway through.
      markScheduleEvaluated(name, entry.task.schedule, scheduledFor ?? Date.now());
      const run = recordRunStart(name, trigger, runId, scheduledFor);
      this.running.add(name);
      startRunLogCapture(run.runId, name);
      taskRunBus.emit({ type: "run-started", taskName: name });
      try {
        await runWithLogCapture(run.runId, () =>
          trigger === "manual" && manualInput !== undefined
            ? (entry.task as ScheduledTask & HandlesManualRunInput).runManual(
                manualInput,
              )
            : entry.task.run(),
        );
        recordRunEnd(run.runId, {
          status: "success",
          summary: providesRunSummary(entry.task)
            ? entry.task.getLastRunSummary()
            : undefined,
        });
      } catch (error) {
        recordRunEnd(run.runId, {
          status: "error",
          error: error instanceof Error ? error.message : String(error),
        });
        throw error;
      } finally {
        // After recordRunEnd, so log-stream clients see the settled status.
        finishRunLogCapture(run.runId);
        this.running.delete(name);
        taskRunBus.emit({ type: "run-finished", taskName: name });
      }
    });
  }

  public list(): TaskInfo[] {
    return [...this.tasks.entries()].map(([name, entry]) => ({
      name,
      displayName: getDisplayName(entry.task),
      schedule: entry.task.schedule,
      running: this.running.has(name),
      nextRuns: this.getNextRuns(entry.cronTask),
      lastRun: getLastRun(name) ?? null,
    }));
  }

  private getNextRuns(cronTask: CronScheduledTask): string[] {
    try {
      return cronTask.getNextRuns(3).map((d) => d.toISOString());
    } catch {
      return [];
    }
  }
}

export class TaskNotFoundError extends Error {
  constructor(name: string) {
    super(`Unknown task "${name}"`);
  }
}

export class TaskAlreadyRunningError extends Error {
  constructor(name: string) {
    super(`Task "${name}" is already running`);
  }
}

export class TaskManualInputUnsupportedError extends Error {
  constructor(name: string) {
    super(`Task "${name}" does not accept manual input`);
  }
}
