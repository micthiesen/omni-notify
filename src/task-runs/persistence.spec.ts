import { describe, expect, it } from "vitest";
import { selectRunsToPrune, type TaskRunData } from "./persistence.js";

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
