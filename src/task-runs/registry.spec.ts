import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect, Fiber } from "effect";
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import {
  getLastRun,
  getTaskScheduleState,
  markScheduleEvaluated,
  recordRunStart,
  TaskRunEntity,
  TaskRunLogEntity,
  TaskScheduleStateEntity,
} from "./persistence.js";
import {
  type TaskServices,
  TaskAlreadyRunningError,
  TaskManualInputUnsupportedError,
  TaskRegistry,
} from "./registry.js";

const mitools = createMitoolsTestRuntime();
const logger = mitools.logger;
afterAll(() => mitools.dispose());

class FakeTask implements ScheduledTask<unknown, TaskServices> {
  public runs = 0;
  public readonly run = Effect.sync(() => {
    this.runs++;
  });

  public constructor(
    public readonly name: string,
    public readonly schedule: string,
    public readonly runOnStartup = false,
  ) {}
}

class ManualInputTask extends FakeTask {
  public inputs: unknown[] = [];
  public runManual(input: unknown): Effect.Effect<void, unknown> {
    return Effect.sync(() => this.inputs.push(input)).pipe(Effect.asVoid);
  }
}

class FailingManualTask extends ManualInputTask {
  public override runManual(input: unknown) {
    return Effect.sync(() => this.inputs.push(input)).pipe(
      Effect.andThen(Effect.fail(new Error("workspace run failed"))),
    );
  }
}

class NativeEffectTask extends FakeTask {
  public started = false;
  public finalized = false;
  public override readonly run = Effect.sync(() => {
    this.started = true;
  }).pipe(
    Effect.andThen(Effect.never),
    Effect.ensuring(
      Effect.sync(() => {
        this.finalized = true;
      }),
    ),
  );
}

function localTime(day: number, hour: number): number {
  return new Date(2026, 6, day, hour).getTime();
}

describe("TaskRegistry missed-run recovery", () => {
  beforeEach(async () => {
    await mitools.run(
      Effect.all([
        TaskRunEntity.deleteAll(),
        TaskRunLogEntity.deleteAll(),
        TaskScheduleStateEntity.deleteAll(),
      ]),
    );
    vi.spyOn(console, "info").mockImplementation(() => {});
  });

  afterEach(() => vi.restoreAllMocks());

  it("repairs interrupted runs through explicit initialization", async () => {
    const running = await mitools.run(recordRunStart("Interrupted", "schedule"));
    const registry = new TaskRegistry(logger);
    await mitools.run(registry.initializeEffect());
    await expect(
      mitools.run(TaskRunEntity.get({ runId: running.runId })),
    ).resolves.toMatchObject({
      value: { status: "error", error: "interrupted (process exited)" },
    });
  });

  it("establishes a baseline without running a never-seen task", async () => {
    const now = localTime(15, 10);
    const task = new FakeTask("Daily", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    await mitools.run(registry.recoverMissedTasksEffect(now));
    expect(task.runs).toBe(0);
    await expect(mitools.run(getTaskScheduleState(task.name))).resolves.toEqual({
      taskName: task.name,
      schedule: task.schedule,
      evaluatedThrough: now,
    });
  });

  it("records an eligible recovery with its original scheduled time", async () => {
    const now = localTime(15, 10);
    const task = new FakeTask("Daily", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    await mitools.run(
      markScheduleEvaluated(task.name, task.schedule, localTime(14, 6)),
    );
    await mitools.run(registry.recoverMissedTasksEffect(now));
    expect(task.runs).toBe(1);
    await expect(mitools.run(getLastRun(task.name))).resolves.toMatchObject({
      taskName: task.name,
      trigger: "catchup",
      scheduledFor: localTime(15, 5),
      status: "success",
    });
    await expect(mitools.run(getTaskScheduleState(task.name))).resolves.toMatchObject({
      evaluatedThrough: localTime(15, 5),
    });
  });

  it("does not add recovery on top of a task that runs on startup", async () => {
    const now = localTime(15, 10);
    const task = new FakeTask("StartupDaily", "0 0 5 * * *", true);
    const registry = new TaskRegistry(logger);
    registry.track(task);
    await mitools.run(
      markScheduleEvaluated(task.name, task.schedule, localTime(14, 6)),
    );
    await mitools.run(registry.recoverMissedTasksEffect(now));
    expect(task.runs).toBe(0);
    await expect(mitools.run(getTaskScheduleState(task.name))).resolves.toMatchObject({
      evaluatedThrough: now,
    });
  });

  it("resets the baseline when a task's schedule changes", async () => {
    const now = localTime(15, 10);
    const task = new FakeTask("Changed", "0 0 6 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    await mitools.run(
      markScheduleEvaluated(task.name, "0 0 5 * * *", localTime(14, 6)),
    );
    await mitools.run(registry.recoverMissedTasksEffect(now));
    expect(task.runs).toBe(0);
    await expect(mitools.run(getTaskScheduleState(task.name))).resolves.toEqual({
      taskName: task.name,
      schedule: task.schedule,
      evaluatedThrough: now,
    });
  });

  it("passes optional input only to tasks that handle manual runs", async () => {
    const task = new ManualInputTask("Parameterized", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    await mitools.run(registry.runNowAndWaitEffect(task.name, { count: 5 }));
    expect(task.inputs).toEqual([{ count: 5 }]);
    expect(task.runs).toBe(0);
  });

  it("rejects manual input for ordinary scheduled tasks", async () => {
    const task = new FakeTask("Ordinary", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    await expect(
      mitools.run(registry.runNow(task.name, { count: 5 })),
    ).rejects.toBeInstanceOf(TaskManualInputUnsupportedError);
  });

  it("atomically rejects one of two simultaneous manual runs", async () => {
    const task = new NativeEffectTask("ConcurrentManual", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    const results = await mitools.run(
      Effect.all(
        [
          Effect.result(registry.runNow(task.name)),
          Effect.result(registry.runNow(task.name)),
        ],
        { concurrency: "unbounded" },
      ),
    );
    expect(results.filter((result) => result._tag === "Success")).toHaveLength(1);
    const failure = results.find((result) => result._tag === "Failure");
    expect(failure?._tag === "Failure" ? failure.failure : undefined).toBeInstanceOf(
      TaskAlreadyRunningError,
    );
    await mitools.run(registry.shutdownEffect());
  });

  it("waits for the exact manual run to finish successfully", async () => {
    const task = new ManualInputTask("Awaited", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    const result = await mitools.run(
      registry.runNowAndWaitEffect(task.name, { source: "email" }),
    );
    expect(task.inputs).toEqual([{ source: "email" }]);
    await expect(mitools.run(getLastRun(task.name))).resolves.toMatchObject({
      runId: result.runId,
      status: "success",
    });
  });

  it("fails only after the awaited run is durably recorded as failed", async () => {
    const task = new FailingManualTask("AwaitedFailure", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    await expect(
      mitools.run(registry.runNowAndWaitEffect(task.name, { source: "email" })),
    ).rejects.toThrow();
    await expect(mitools.run(getLastRun(task.name))).resolves.toMatchObject({
      status: "error",
    });
  });

  it("interrupts the native task Effect and records the stopped run", async () => {
    const task = new NativeEffectTask("Native", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    const fiber = mitools.runFork(registry.runNowAndWaitEffect(task.name));
    await vi.waitFor(() => expect(task.started).toBe(true));
    await mitools.run(Fiber.interrupt(fiber));
    expect(task.finalized).toBe(true);
    await expect(mitools.run(getLastRun(task.name))).resolves.toMatchObject({
      status: "error",
    });
  });
});
