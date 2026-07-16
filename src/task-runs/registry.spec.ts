import { Injector } from "@micthiesen/mitools/config";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  getLastRun,
  getTaskScheduleState,
  markScheduleEvaluated,
  TaskRunEntity,
  TaskRunLogEntity,
  TaskScheduleStateEntity,
} from "./persistence.js";
import { TaskRegistry } from "./registry.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "task-registry.spec.db",
  },
});

const logger = new Logger("Test");

class FakeTask extends ScheduledTask {
  public runs = 0;

  public constructor(
    public readonly name: string,
    public readonly schedule: string,
    public override readonly runOnStartup = false,
  ) {
    super();
  }

  public async run(): Promise<void> {
    this.runs++;
  }
}

function localTime(day: number, hour: number): number {
  return new Date(2026, 6, day, hour).getTime();
}

describe("TaskRegistry missed-run recovery", () => {
  beforeEach(() => {
    TaskRunEntity.deleteAll();
    TaskRunLogEntity.deleteAll();
    TaskScheduleStateEntity.deleteAll();
    vi.spyOn(console, "info").mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("establishes a baseline without running a never-seen task", async () => {
    const now = localTime(15, 10);
    const task = new FakeTask("Daily", "0 0 5 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);

    await registry.recoverMissedTasks(now);

    expect(task.runs).toBe(0);
    expect(getTaskScheduleState(task.name)).toEqual({
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
    markScheduleEvaluated(task.name, task.schedule, localTime(14, 6));

    await registry.recoverMissedTasks(now);

    expect(task.runs).toBe(1);
    expect(getLastRun(task.name)).toMatchObject({
      taskName: task.name,
      trigger: "catchup",
      scheduledFor: localTime(15, 5),
      status: "success",
    });
    expect(getTaskScheduleState(task.name)?.evaluatedThrough).toBe(localTime(15, 5));
  });

  it("does not add recovery on top of a task that runs on startup", async () => {
    const now = localTime(15, 10);
    const task = new FakeTask("StartupDaily", "0 0 5 * * *", true);
    const registry = new TaskRegistry(logger);
    registry.track(task);
    markScheduleEvaluated(task.name, task.schedule, localTime(14, 6));

    await registry.recoverMissedTasks(now);

    expect(task.runs).toBe(0);
    expect(getTaskScheduleState(task.name)?.evaluatedThrough).toBe(now);
  });

  it("resets the baseline when a task's schedule changes", async () => {
    const now = localTime(15, 10);
    const task = new FakeTask("Changed", "0 0 6 * * *");
    const registry = new TaskRegistry(logger);
    registry.track(task);
    markScheduleEvaluated(task.name, "0 0 5 * * *", localTime(14, 6));

    await registry.recoverMissedTasks(now);

    expect(task.runs).toBe(0);
    expect(getTaskScheduleState(task.name)).toEqual({
      taskName: task.name,
      schedule: task.schedule,
      evaluatedThrough: now,
    });
  });
});
