import { Docstore } from "@micthiesen/mitools/docstore";
import { Logger, type NamedLogger } from "@micthiesen/mitools/logging";
import { Karakeep } from "@micthiesen/mitools/karakeep";
import { Pushover } from "@micthiesen/mitools/pushover";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Sqlite } from "@micthiesen/mitools/sqlite";
import { Cause, Clock, Data, Effect, Exit, Fiber, Semaphore } from "effect";
import cron, { type ScheduledTask as CronScheduledTask } from "node-cron";
import { decideCatchUp } from "./catchUp.js";
import { taskRunBus } from "./events.js";
import {
  finishRunLogCapture,
  runWithLogCaptureEffect,
  startRunLogCapture,
} from "./logCapture.js";
import {
  getLastRun,
  getTaskScheduleState,
  makeRunId,
  markInterruptedRuns,
  markScheduleEvaluated,
  recordRunEnd,
  recordRunStartAndMarkSchedule,
  type TaskRunData,
  type TaskRunTrigger,
} from "./persistence.js";

/** Tasks may report a one-line summary of their most recent run. */
interface ProvidesRunSummary {
  getLastRunSummary(): string | undefined;
}

interface HandlesManualRunInput {
  runManual(input: unknown): Effect.Effect<void, unknown, TaskServices>;
}

export type TaskServices = Logger | Docstore | Pushover | Karakeep | Sqlite;
export type AppScheduledTask = ScheduledTask<unknown, TaskServices>;

/** Tasks may report a friendlier name for the UI; `name` itself stays the load-bearing key. */
interface HasDisplayName {
  displayName?: string;
}

function providesRunSummary(
  task: AppScheduledTask,
): task is AppScheduledTask & ProvidesRunSummary {
  return typeof (task as Partial<ProvidesRunSummary>).getLastRunSummary === "function";
}

function handlesManualRunInput(
  task: AppScheduledTask,
): task is AppScheduledTask & HandlesManualRunInput {
  return typeof (task as Partial<HandlesManualRunInput>).runManual === "function";
}

function getDisplayName(task: AppScheduledTask): string | undefined {
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
    {
      task: AppScheduledTask;
      semaphore: Semaphore.Semaphore;
      cronTask: CronScheduledTask;
    }
  >();
  private running = new Set<string>();
  private queued = new Map<string, number>();
  private hasRun = new Set<string>();
  private backgroundFibers = new Set<Fiber.Fiber<void, unknown>>();
  private logger: NamedLogger;
  private initialized = false;

  constructor(parentLogger: NamedLogger) {
    this.logger = parentLogger.extend("TaskRegistry");
  }

  public readonly initializeEffect = Effect.fn("TaskRegistry.initialize")(
    function* (this: TaskRegistry) {
      if (this.initialized) return;
      const interrupted = yield* markInterruptedRuns();
      this.initialized = true;
      if (interrupted > 0) {
        yield* this.logger.warn(
          `Marked ${interrupted} interrupted task run(s) as errors`,
        );
      }
    },
  );

  /**
   * Wrap a task for the Scheduler. The wrapper funnels scheduled executions
   * through this registry's per-task queue, alongside manual runs.
   */
  public track(task: AppScheduledTask): AppScheduledTask {
    if (this.tasks.has(task.name)) {
      throw new Error(`Task "${task.name}" is already registered`);
    }
    // Never-started cron instance, used purely to compute upcoming run times.
    const cronTask = cron.createTask(task.schedule, () => {});
    const semaphore = Semaphore.makeUnsafe(1);
    this.tasks.set(task.name, { task, semaphore, cronTask });

    return {
      name: task.name,
      schedule: task.schedule,
      jitterMs: task.jitterMs,
      runOnStartup: task.runOnStartup,
      run: this.queuedExecutionEffect(task.name, "schedule"),
    };
  }

  /** Queue a manual run. Rejects immediately if the task is already running. */
  public runNow(name: string, input?: unknown) {
    return Effect.uninterruptible(
      Effect.gen({ self: this }, function* () {
        const entry = this.tasks.get(name);
        if (!entry) return yield* new TaskNotFoundError({ name });
        if (input !== undefined && !handlesManualRunInput(entry.task)) {
          return yield* new TaskManualInputUnsupportedError({ name });
        }
        if (this.running.has(name) || (this.queued.get(name) ?? 0) > 0) {
          return yield* new TaskAlreadyRunningError({ name });
        }
        const runId = makeRunId(name);
        this.reserveQueued(name);
        const fiber = yield* Effect.forkDetach(
          this.reservedExecutionEffect(name, "manual", runId, undefined, input).pipe(
            Effect.catchCause((cause) =>
              this.logger.error(`Manual run of "${name}" failed`, cause),
            ),
          ),
        );
        this.backgroundFibers.add(fiber);
        fiber.addObserver(() => this.backgroundFibers.delete(fiber));
        return { runId };
      }),
    );
  }

  /**
   * Queue a manual run and keep the caller in the same structured Effect until
   * that exact run has durably finished. Unlike the UI's fire-and-forget
   * `runNow`, this deliberately waits behind an active run.
   */
  public runNowAndWaitEffect(name: string, input?: unknown) {
    return Effect.suspend(() => {
      const entry = this.tasks.get(name);
      if (!entry) return Effect.fail(new TaskNotFoundError({ name }));
      if (input !== undefined && !handlesManualRunInput(entry.task)) {
        return Effect.fail(new TaskManualInputUnsupportedError({ name }));
      }
      const runId = makeRunId(name);
      return this.queuedExecutionEffect(name, "manual", runId, undefined, input).pipe(
        Effect.as({ runId }),
      );
    });
  }

  /** Interrupt task runs started outside the Scheduler during app shutdown. */
  public shutdownEffect() {
    return Fiber.interruptAll([...this.backgroundFibers]);
  }

  /** Recover the newest eligible cron occurrence for each infrequent task. */
  public recoverMissedTasksEffect(now?: number) {
    return Effect.gen({ self: this }, function* () {
      const currentTime = now ?? (yield* Clock.currentTimeMillis);
      const recoveries: { name: string; scheduledFor: number }[] = [];

      for (const [name, entry] of this.tasks) {
        const state = yield* getTaskScheduleState(name);
        if (state && state.schedule !== entry.task.schedule) {
          yield* this.logger.info(
            `Schedule changed for "${name}"; starting a new recovery baseline`,
          );
          yield* markScheduleEvaluated(name, entry.task.schedule, currentTime);
          continue;
        }

        const lastRun = yield* getLastRun(name);
        const evaluatedThrough = state?.evaluatedThrough ?? lastRun?.startedAt;
        if (evaluatedThrough === undefined || entry.task.runOnStartup) {
          yield* markScheduleEvaluated(name, entry.task.schedule, currentTime);
          continue;
        }

        const decision = decideCatchUp(
          entry.task.schedule,
          evaluatedThrough,
          currentTime,
        );
        switch (decision.kind) {
          case "run":
            recoveries.push({ name, scheduledFor: decision.scheduledFor });
            break;
          case "stale":
            yield* this.logger.info(
              `Skipping stale missed run of "${name}" from ${new Date(decision.scheduledFor).toISOString()}`,
            );
            yield* markScheduleEvaluated(name, entry.task.schedule, currentTime);
            break;
          case "disabled":
          case "none":
            yield* markScheduleEvaluated(name, entry.task.schedule, currentTime);
            break;
        }
      }

      // Recover sequentially so a reboot cannot unleash several expensive tasks at once.
      yield* Effect.forEach(
        recoveries,
        (recovery) => {
          return this.logger
            .info(
              `Recovering missed run of "${recovery.name}" from ${new Date(recovery.scheduledFor).toISOString()}`,
            )
            .pipe(
              Effect.andThen(
                this.executeEffect(
                  recovery.name,
                  "catchup",
                  undefined,
                  recovery.scheduledFor,
                ),
              ),
              Effect.catch((error) =>
                this.logger.error(`Catch-up run of "${recovery.name}" failed`, error),
              ),
            );
        },
        { concurrency: 1, discard: true },
      );
    });
  }

  private queuedExecutionEffect(
    name: string,
    trigger: TaskRunTrigger,
    runId?: string,
    scheduledFor?: number,
    manualInput?: unknown,
  ) {
    return Effect.suspend(() => {
      this.reserveQueued(name);
      return this.reservedExecutionEffect(
        name,
        trigger,
        runId,
        scheduledFor,
        manualInput,
      );
    });
  }

  private reserveQueued(name: string): void {
    this.queued.set(name, (this.queued.get(name) ?? 0) + 1);
  }

  private reservedExecutionEffect(
    name: string,
    trigger: TaskRunTrigger,
    runId?: string,
    scheduledFor?: number,
    manualInput?: unknown,
  ) {
    return this.executeEffect(name, trigger, runId, scheduledFor, manualInput).pipe(
      Effect.ensuring(
        Effect.sync(() => {
          const remaining = (this.queued.get(name) ?? 1) - 1;
          if (remaining > 0) this.queued.set(name, remaining);
          else this.queued.delete(name);
        }),
      ),
    );
  }

  private executeEffect(
    name: string,
    trigger: TaskRunTrigger,
    runId?: string,
    scheduledFor?: number,
    manualInput?: unknown,
  ) {
    const entry = this.tasks.get(name);
    if (!entry) return Effect.fail(new TaskNotFoundError({ name }));

    return entry.semaphore.withPermits(1)(
      Effect.gen({ self: this }, function* () {
        // The Scheduler fires runOnStartup tasks through the same path as cron
        // runs. Do not consume startup state until durable run creation succeeds.
        const actualTrigger =
          trigger === "schedule" && entry.task.runOnStartup && !this.hasRun.has(name)
            ? "startup"
            : trigger;
        // Establish the durable run and catch-up cursor atomically. A crash can
        // leave an interrupted run, but can never advance the cursor without a
        // corresponding run row.
        const startedAt = yield* Effect.clockWith((clock) => clock.currentTimeMillis);
        const run = yield* recordRunStartAndMarkSchedule(
          name,
          actualTrigger,
          entry.task.schedule,
          scheduledFor ?? startedAt,
          runId,
          scheduledFor,
          startedAt,
        );
        this.hasRun.add(name);
        this.running.add(name);
        startRunLogCapture(run.runId, name);
        yield* taskRunBus.emitEffect({ type: "run-started", taskName: name });

        const taskEffect = runWithLogCaptureEffect(
          run.runId,
          actualTrigger === "manual" && manualInput !== undefined
            ? (entry.task as AppScheduledTask & HandlesManualRunInput).runManual(
                manualInput,
              )
            : entry.task.run,
        );

        yield* taskEffect.pipe(
          Effect.onExit((exit) =>
            Effect.clockWith((clock) => clock.currentTimeMillis).pipe(
              Effect.flatMap((finishedAt) =>
                recordRunEnd(
                  run.runId,
                  {
                    status: Exit.isSuccess(exit) ? "success" : "error",
                    error: Exit.isFailure(exit) ? Cause.pretty(exit.cause) : undefined,
                    summary: providesRunSummary(entry.task)
                      ? entry.task.getLastRunSummary()
                      : undefined,
                  },
                  finishedAt,
                ),
              ),
            ),
          ),
          Effect.ensuring(this.finishRunEffect(run.runId, name)),
        );
      }),
    );
  }

  private finishRunEffect(runId: string, name: string) {
    const safely = (
      operation: string,
      action: Effect.Effect<void, unknown, TaskServices>,
    ) =>
      action.pipe(
        Effect.catch((error) => this.logger.error(`${operation} failed`, error)),
      );
    return Effect.all(
      [
        safely("Finish task log capture", finishRunLogCapture(runId)),
        Effect.sync(() => this.running.delete(name)),
        taskRunBus.emitEffect({ type: "run-finished", taskName: name }),
      ],
      { concurrency: "unbounded", discard: true },
    );
  }

  public list() {
    return Effect.forEach([...this.tasks.entries()], ([name, entry]) =>
      getLastRun(name).pipe(
        Effect.map(
          (lastRun) =>
            ({
              name,
              displayName: getDisplayName(entry.task),
              schedule: entry.task.schedule,
              running: this.running.has(name),
              nextRuns: this.getNextRuns(entry.cronTask),
              lastRun: lastRun ?? null,
            }) satisfies TaskInfo,
        ),
      ),
    );
  }

  private getNextRuns(cronTask: CronScheduledTask): string[] {
    try {
      return cronTask.getNextRuns(3).map((d) => d.toISOString());
    } catch {
      return [];
    }
  }
}

export class TaskNotFoundError extends Data.TaggedError("TaskNotFoundError")<{
  readonly name: string;
}> {
  public override get message(): string {
    return `Unknown task "${this.name}"`;
  }
}

export class TaskAlreadyRunningError extends Data.TaggedError(
  "TaskAlreadyRunningError",
)<{ readonly name: string }> {
  public override get message(): string {
    return `Task "${this.name}" is already running`;
  }
}

export class TaskManualInputUnsupportedError extends Data.TaggedError(
  "TaskManualInputUnsupportedError",
)<{ readonly name: string }> {
  public override get message(): string {
    return `Task "${this.name}" does not accept manual input`;
  }
}
