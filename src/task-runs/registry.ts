import type { Effect as EffectType } from "effect/Effect";
import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Cause, Data, Effect, Exit, Fiber, Semaphore } from "effect";
import cron, { type ScheduledTask as CronScheduledTask } from "node-cron";
import { IntegrationError, type PersistenceError } from "../effect/errors.js";
import { fromPromise, fromSync, runPromise } from "../effect/interop.js";
import { decideCatchUp } from "./catchUp.js";
import { taskRunBus } from "./events.js";
import {
  finishRunLogCapture,
  runWithLogCapture,
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
  runManual(input: unknown): Promise<void>;
}

interface RunsEffect {
  runEffect(): EffectType<void, unknown>;
}

interface HandlesManualRunInputEffect {
  runManualEffect(input: unknown): EffectType<void, unknown>;
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

function runsEffect(task: ScheduledTask): task is ScheduledTask & RunsEffect {
  return typeof (task as Partial<RunsEffect>).runEffect === "function";
}

function handlesManualRunInputEffect(
  task: ScheduledTask,
): task is ScheduledTask & HandlesManualRunInputEffect {
  return (
    typeof (task as Partial<HandlesManualRunInputEffect>).runManualEffect === "function"
  );
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
    {
      task: ScheduledTask;
      semaphore: Semaphore.Semaphore;
      cronTask: CronScheduledTask;
    }
  >();
  private running = new Set<string>();
  private queued = new Map<string, number>();
  private hasRun = new Set<string>();
  private backgroundFibers = new Set<Fiber.Fiber<void, unknown>>();
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
    const semaphore = Semaphore.makeUnsafe(1);
    this.tasks.set(task.name, { task, semaphore, cronTask });

    const executeScheduled = () => this.execute(task.name, "schedule");
    return new (class extends ScheduledTask {
      public readonly name = task.name;
      public readonly schedule = task.schedule;
      public override readonly jitterMs = task.jitterMs;
      public override readonly runOnStartup = task.runOnStartup;
      public run(): Promise<void> {
        return executeScheduled();
      }
    })();
  }

  /** Queue a manual run. Rejects immediately if the task is already running. */
  public runNow(name: string, input?: unknown): { runId: string } {
    const entry = this.tasks.get(name);
    if (!entry) throw new TaskNotFoundError({ name });
    if (input !== undefined && !handlesManualRunInput(entry.task)) {
      throw new TaskManualInputUnsupportedError({ name });
    }
    if (this.running.has(name) || (this.queued.get(name) ?? 0) > 0) {
      throw new TaskAlreadyRunningError({ name });
    }
    const runId = makeRunId(name);
    const fiber = Effect.runFork(
      this.queuedExecutionEffect(name, "manual", runId, undefined, input).pipe(
        Effect.catchCause((cause) =>
          Effect.sync(() => this.logger.error(`Manual run of "${name}" failed`, cause)),
        ),
      ),
    );
    this.backgroundFibers.add(fiber);
    fiber.addObserver(() => this.backgroundFibers.delete(fiber));
    return { runId };
  }

  /**
   * Queue a manual run and keep the caller in the same structured Effect until
   * that exact run has durably finished. Unlike the UI's fire-and-forget
   * `runNow`, this deliberately waits behind an active run.
   */
  public runNowAndWaitEffect(
    name: string,
    input?: unknown,
  ): EffectType<
    { runId: string },
    | TaskNotFoundError
    | TaskManualInputUnsupportedError
    | PersistenceError
    | IntegrationError
  > {
    return Effect.suspend(
      (): EffectType<
        { runId: string },
        | TaskNotFoundError
        | TaskManualInputUnsupportedError
        | PersistenceError
        | IntegrationError
      > => {
        const entry = this.tasks.get(name);
        if (!entry) return Effect.fail(new TaskNotFoundError({ name }));
        if (input !== undefined && !handlesManualRunInput(entry.task)) {
          return Effect.fail(new TaskManualInputUnsupportedError({ name }));
        }
        const runId = makeRunId(name);
        return this.queuedExecutionEffect(name, "manual", runId, undefined, input).pipe(
          Effect.as({ runId }),
        );
      },
    );
  }

  /** Interrupt task runs started outside the Scheduler during app shutdown. */
  public shutdownEffect(): EffectType<void> {
    return Fiber.interruptAll([...this.backgroundFibers]);
  }

  /** Recover the newest eligible cron occurrence for each infrequent task. */
  public recoverMissedTasksEffect(
    now = Date.now(),
  ): EffectType<void, PersistenceError | TaskNotFoundError> {
    return Effect.gen({ self: this }, function* () {
      const recoveries: { name: string; scheduledFor: number }[] = [];

      for (const [name, entry] of this.tasks) {
        const state = yield* fromSync("read task schedule state", () =>
          getTaskScheduleState(name),
        );
        if (state && state.schedule !== entry.task.schedule) {
          this.logger.info(
            `Schedule changed for "${name}"; starting a new recovery baseline`,
          );
          yield* fromSync("reset task recovery baseline", () =>
            markScheduleEvaluated(name, entry.task.schedule, now),
          );
          continue;
        }

        const lastRun = yield* fromSync("read last task run", () => getLastRun(name));
        const evaluatedThrough = state?.evaluatedThrough ?? lastRun?.startedAt;
        if (evaluatedThrough === undefined || entry.task.runOnStartup) {
          yield* fromSync("establish task recovery baseline", () =>
            markScheduleEvaluated(name, entry.task.schedule, now),
          );
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
            yield* fromSync("skip stale task recovery", () =>
              markScheduleEvaluated(name, entry.task.schedule, now),
            );
            break;
          case "disabled":
          case "none":
            yield* fromSync("advance task recovery cursor", () =>
              markScheduleEvaluated(name, entry.task.schedule, now),
            );
            break;
        }
      }

      // Recover sequentially so a reboot cannot unleash several expensive tasks at once.
      yield* Effect.forEach(
        recoveries,
        (recovery) => {
          this.logger.info(
            `Recovering missed run of "${recovery.name}" from ${new Date(recovery.scheduledFor).toISOString()}`,
          );
          return this.executeEffect(
            recovery.name,
            "catchup",
            undefined,
            recovery.scheduledFor,
          ).pipe(
            Effect.catch((error) =>
              Effect.sync(() => {
                this.logger.error(`Catch-up run of "${recovery.name}" failed`, error);
              }),
            ),
          );
        },
        { concurrency: 1, discard: true },
      );
    });
  }

  public recoverMissedTasks(now = Date.now()): Promise<void> {
    return runPromise(this.recoverMissedTasksEffect(now));
  }

  private execute(
    name: string,
    trigger: TaskRunTrigger,
    runId?: string,
    scheduledFor?: number,
    manualInput?: unknown,
  ): Promise<void> {
    return runPromise(
      this.queuedExecutionEffect(name, trigger, runId, scheduledFor, manualInput),
    );
  }

  private queuedExecutionEffect(
    name: string,
    trigger: TaskRunTrigger,
    runId?: string,
    scheduledFor?: number,
    manualInput?: unknown,
  ): EffectType<void, TaskNotFoundError | PersistenceError | IntegrationError> {
    this.queued.set(name, (this.queued.get(name) ?? 0) + 1);
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
  ): EffectType<void, TaskNotFoundError | PersistenceError | IntegrationError> {
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
        const run = yield* fromSync("record task run start", () =>
          recordRunStartAndMarkSchedule(
            name,
            actualTrigger,
            entry.task.schedule,
            scheduledFor ?? Date.now(),
            runId,
            scheduledFor,
          ),
        );
        this.hasRun.add(name);
        this.running.add(name);
        startRunLogCapture(run.runId, name);
        yield* taskRunBus.emitEffect({ type: "run-started", taskName: name });

        const nativeEffect =
          actualTrigger === "manual" && manualInput !== undefined
            ? handlesManualRunInputEffect(entry.task)
              ? entry.task.runManualEffect(manualInput)
              : undefined
            : runsEffect(entry.task)
              ? entry.task.runEffect()
              : undefined;
        const taskEffect = nativeEffect
          ? runWithLogCaptureEffect(run.runId, nativeEffect).pipe(
              Effect.mapError(
                (cause) =>
                  new IntegrationError({ operation: "run scheduled task", cause }),
              ),
            )
          : fromPromise("run scheduled task", () =>
              runWithLogCapture(run.runId, () =>
                actualTrigger === "manual" && manualInput !== undefined
                  ? (entry.task as ScheduledTask & HandlesManualRunInput).runManual(
                      manualInput,
                    )
                  : entry.task.run(),
              ),
            );

        yield* taskEffect.pipe(
          Effect.onExit((exit) =>
            fromSync(
              Exit.isSuccess(exit)
                ? "record successful task run"
                : "record failed task run",
              () =>
                recordRunEnd(run.runId, {
                  status: Exit.isSuccess(exit) ? "success" : "error",
                  error: Exit.isFailure(exit) ? Cause.pretty(exit.cause) : undefined,
                  summary: providesRunSummary(entry.task)
                    ? entry.task.getLastRunSummary()
                    : undefined,
                }),
            ),
          ),
          Effect.ensuring(this.finishRunEffect(run.runId, name)),
        );
      }),
    );
  }

  private finishRunEffect(runId: string, name: string): EffectType<void> {
    const safely = (operation: string, action: () => void) =>
      Effect.try({ try: action, catch: (cause) => cause }).pipe(
        Effect.catch((error) =>
          Effect.sync(() => this.logger.error(`${operation} failed`, error)),
        ),
      );
    return Effect.all(
      [
        safely("Finish task log capture", () => finishRunLogCapture(runId)),
        Effect.sync(() => this.running.delete(name)),
        taskRunBus.emitEffect({ type: "run-finished", taskName: name }),
      ],
      { concurrency: "unbounded", discard: true },
    );
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
