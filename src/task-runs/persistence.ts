import { gunzipSync, gzipSync } from "node:zlib";
import { Entity } from "@micthiesen/mitools/entities";
import { Logger, type LogLevel } from "@micthiesen/mitools/logging";

const logger = new Logger("TaskRuns");

export type TaskRunTrigger = "schedule" | "manual" | "startup" | "catchup";
export type TaskRunStatus = "running" | "success" | "error";

export type TaskRunData = {
  runId: string;
  taskName: string;
  trigger: TaskRunTrigger;
  /** Cron occurrence this run is recovering, for catch-up runs. */
  scheduledFor?: number;
  startedAt: number;
  finishedAt?: number;
  status: TaskRunStatus;
  error?: string;
  /** Optional one-line result provided by tasks that report one. */
  summary?: string;
};

export const TaskRunEntity = new Entity<TaskRunData, ["runId"]>("task-run", ["runId"]);

export type TaskScheduleStateData = {
  taskName: string;
  schedule: string;
  evaluatedThrough: number;
};

export const TaskScheduleStateEntity = new Entity<TaskScheduleStateData, ["taskName"]>(
  "task-schedule-state",
  ["taskName"],
);

export type TaskRunLogLine = {
  /** Epoch ms of the log call */
  t: number;
  level: LogLevel;
  /** Logger name, e.g. "Main:LiveCheck" */
  logger: string;
  msg: string;
};

export type TaskRunLogData = {
  runId: string;
  taskName: string;
  lines: TaskRunLogLine[];
  /** Oldest lines dropped once the per-run cap was hit. */
  dropped: number;
};

type StoredTaskRunLog = {
  runId: string;
  taskName: string;
  /** Legacy rows written before compression stored the raw lines. */
  lines?: TaskRunLogLine[];
  /** gzip(JSON.stringify(lines)), base64-encoded. */
  linesGz?: string;
  /** Oldest lines dropped once the per-run cap was hit. */
  dropped: number;
};

/** One row per finished run; written once at run end, pruned with the run. */
export const TaskRunLogEntity = new Entity<StoredTaskRunLog, ["runId"]>(
  "task-run-log",
  ["runId"],
);

const KEEP_PER_TASK = 50;

export function makeRunId(taskName: string): string {
  return `${taskName}:${Date.now()}:${Math.random().toString(36).slice(2, 8)}`;
}

export function recordRunStart(
  taskName: string,
  trigger: TaskRunTrigger,
  runId: string = makeRunId(taskName),
  scheduledFor?: number,
): TaskRunData {
  const run: TaskRunData = {
    runId,
    taskName,
    trigger,
    scheduledFor,
    startedAt: Date.now(),
    status: "running",
  };
  TaskRunEntity.upsert(run);
  pruneRuns(taskName);
  return run;
}

export function getTaskScheduleState(
  taskName: string,
): TaskScheduleStateData | undefined {
  return TaskScheduleStateEntity.get({ taskName }) ?? undefined;
}

export function markScheduleEvaluated(
  taskName: string,
  schedule: string,
  evaluatedThrough: number,
): void {
  TaskScheduleStateEntity.upsert({ taskName, schedule, evaluatedThrough });
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

export function getRun(runId: string): TaskRunData | undefined {
  return TaskRunEntity.get({ runId });
}

/** gzip(JSON.stringify(lines)) as base64, the stored form of captured logs. */
export function compressLogLines(lines: TaskRunLogLine[]): string {
  return gzipSync(JSON.stringify(lines)).toString("base64");
}

export function decompressLogLines(linesGz: string): TaskRunLogLine[] {
  return JSON.parse(
    gunzipSync(Buffer.from(linesGz, "base64")).toString("utf8"),
  ) as TaskRunLogLine[];
}

export function saveRunLogs(data: TaskRunLogData): void {
  if (data.lines.length === 0 && data.dropped === 0) return;
  TaskRunLogEntity.upsert({
    runId: data.runId,
    taskName: data.taskName,
    linesGz: compressLogLines(data.lines),
    dropped: data.dropped,
  });
}

export function getRunLogs(runId: string): TaskRunLogData | undefined {
  try {
    const row = TaskRunLogEntity.get({ runId });
    if (!row) return undefined;
    const lines = row.linesGz ? decompressLogLines(row.linesGz) : (row.lines ?? []);
    return { runId: row.runId, taskName: row.taskName, lines, dropped: row.dropped };
  } catch (err) {
    // A truncated/corrupt CBOR blob (e.g. a row half-written when the
    // container was killed mid-deploy) would otherwise 500 the logs endpoint
    // forever. Drop the unreadable row and treat the run as having no logs.
    TaskRunLogEntity.delete({ runId });
    logger.warn(`Dropped unreadable log row for run "${runId}": ${String(err)}`);
    return undefined;
  }
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
    TaskRunLogEntity.delete({ runId: run.runId });
  }
}
