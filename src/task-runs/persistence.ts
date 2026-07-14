import { Entity } from "@micthiesen/mitools/entities";

export type TaskRunTrigger = "schedule" | "manual" | "startup";
export type TaskRunStatus = "running" | "success" | "error";

export type TaskRunData = {
  runId: string;
  taskName: string;
  trigger: TaskRunTrigger;
  startedAt: number;
  finishedAt?: number;
  status: TaskRunStatus;
  error?: string;
  /** Optional one-line result provided by tasks that report one. */
  summary?: string;
};

export const TaskRunEntity = new Entity<TaskRunData, ["runId"]>("task-run", ["runId"]);

const KEEP_PER_TASK = 50;

export function makeRunId(taskName: string): string {
  return `${taskName}:${Date.now()}:${Math.random().toString(36).slice(2, 8)}`;
}

export function recordRunStart(
  taskName: string,
  trigger: TaskRunTrigger,
  runId: string = makeRunId(taskName),
): TaskRunData {
  const run: TaskRunData = {
    runId,
    taskName,
    trigger,
    startedAt: Date.now(),
    status: "running",
  };
  TaskRunEntity.upsert(run);
  pruneRuns(taskName);
  return run;
}

export function recordRunEnd(
  runId: string,
  result: { status: "success" | "error"; error?: string; summary?: string },
): void {
  TaskRunEntity.patch(
    { runId },
    {
      status: result.status,
      error: result.error,
      summary: result.summary,
      finishedAt: Date.now(),
    },
  );
}

/** Flip runs left in "running" by a crashed process to errors. Call at boot. */
export function markInterruptedRuns(): number {
  const interrupted = TaskRunEntity.getAll().filter((r) => r.status === "running");
  for (const run of interrupted) {
    TaskRunEntity.patch(
      { runId: run.runId },
      {
        status: "error",
        error: "interrupted (process exited)",
        finishedAt: Date.now(),
      },
    );
  }
  return interrupted.length;
}

export function getRuns(taskName?: string, limit = 50): TaskRunData[] {
  const all = TaskRunEntity.getAll()
    .filter((r) => !taskName || r.taskName === taskName)
    .sort((a, b) => b.startedAt - a.startedAt);
  return all.slice(0, limit);
}

export function getLastRun(taskName: string): TaskRunData | undefined {
  return getRuns(taskName, 1)[0];
}

/** Runs beyond the newest `keep` for their task, i.e. the ones to delete. */
export function selectRunsToPrune(
  runs: TaskRunData[],
  taskName: string,
  keep: number,
): TaskRunData[] {
  return runs
    .filter((r) => r.taskName === taskName)
    .sort((a, b) => b.startedAt - a.startedAt)
    .slice(keep);
}

function pruneRuns(taskName: string): void {
  const stale = selectRunsToPrune(TaskRunEntity.getAll(), taskName, KEEP_PER_TASK);
  for (const run of stale) {
    TaskRunEntity.delete({ runId: run.runId });
  }
}
