import { Option } from "effect";
import { afterAll, beforeEach, describe, expect, it } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import {
  getTaskScheduleState,
  recordRunStartAndMarkSchedule,
  selectRunsToPrune,
  TaskRunEntity,
  TaskRunLogEntity,
  type TaskRunData,
} from "./persistence.js";

const mitools = createMitoolsTestRuntime();
afterAll(() => mitools.dispose());
beforeEach(async () => {
  await mitools.run(TaskRunEntity.deleteAll());
  await mitools.run(TaskRunLogEntity.deleteAll());
});

function makeRun(taskName: string, startedAt: number): TaskRunData {
  return {
    runId: `${taskName}:${startedAt}`,
    taskName,
    trigger: "schedule",
    startedAt,
    status: "success",
  };
}

describe("selectRunsToPrune", () => {
  it("keeps the newest N runs for the task", () => {
    const runs = [1, 2, 3, 4, 5].map((i) => makeRun("A", i * 1000));
    const stale = selectRunsToPrune(runs, "A", 3);
    expect(stale.map((r) => r.startedAt)).toEqual([2000, 1000]);
  });

  it("returns nothing at or under the keep limit", () => {
    const runs = [1, 2, 3].map((i) => makeRun("A", i * 1000));
    expect(selectRunsToPrune(runs, "A", 3)).toHaveLength(0);
  });

  it("only considers runs for the given task", () => {
    const runs = [
      ...[1, 2, 3].map((i) => makeRun("A", i * 1000)),
      ...[1, 2, 3].map((i) => makeRun("B", i * 1000)),
    ];
    const stale = selectRunsToPrune(runs, "A", 2);
    expect(stale).toHaveLength(1);
    expect(stale[0].taskName).toBe("A");
  });
});

describe("recordRunStartAndMarkSchedule", () => {
  it("commits the run, cursor, and pruning as one operation", async () => {
    for (let index = 0; index < 50; index++) {
      const run = makeRun("A", index);
      await mitools.run(TaskRunEntity.upsert(run));
      await mitools.run(
        TaskRunLogEntity.upsert({
          runId: run.runId,
          taskName: "A",
          lines: [],
          dropped: 0,
        }),
      );
    }

    const run = await mitools.run(
      recordRunStartAndMarkSchedule(
        "A",
        "catchup",
        "0 0 5 * * *",
        10_000,
        "A:new",
        9_000,
        10_000,
      ),
    );

    expect(run.runId).toBe("A:new");
    expect(await mitools.run(TaskRunEntity.count())).toBe(50);
    expect(await mitools.run(getTaskScheduleState("A"))).toMatchObject({
      evaluatedThrough: 10_000,
    });
    expect(
      Option.getOrUndefined(await mitools.run(TaskRunLogEntity.get({ runId: "A:0" }))),
    ).toBeUndefined();
  });
});
