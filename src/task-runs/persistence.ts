import { gunzipSync, gzipSync } from "node:zlib";
import { randomUUID } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import type { LogLevel, NamedLogger } from "@micthiesen/mitools/logging";
import { Clock, Effect, Option } from "effect";

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
  return `${taskName}:${randomUUID()}`;
}

export const recordRunStart = Effect.fn("TaskRuns.recordStart")(function* (
  taskName: string,
  trigger: TaskRunTrigger,
  runId: string | undefined = undefined,
  scheduledFor?: number,
  startedAt?: number,
) {
  const now = startedAt ?? (yield* Clock.currentTimeMillis);
  const run: TaskRunData = {
    runId: runId ?? makeRunId(taskName),
    taskName,
    trigger,
    scheduledFor,
    startedAt: now,
    status: "running",
  };
  yield* TaskRunEntity.upsert(run);
  yield* pruneRuns(taskName);
  return run;
});

export const getTaskScheduleState = Effect.fn("TaskRuns.getScheduleState")(function* (
  taskName: string,
) {
  return Option.getOrUndefined(yield* TaskScheduleStateEntity.get({ taskName }));
});

export const markScheduleEvaluated = Effect.fn("TaskRuns.markScheduleEvaluated")(
  function* (taskName: string, schedule: string, evaluatedThrough: number) {
    yield* TaskScheduleStateEntity.upsert({ taskName, schedule, evaluatedThrough });
  },
);

/** Atomically establish a durable run before advancing its catch-up cursor. */
export const recordRunStartAndMarkSchedule = Effect.fn(
  "TaskRuns.recordStartAndMarkSchedule",
)(function* (
  taskName: string,
  trigger: TaskRunTrigger,
  schedule: string,
  evaluatedThrough: number,
  runId?: string,
  scheduledFor?: number,
  startedAt?: number,
) {
  const now = startedAt ?? (yield* Clock.currentTimeMillis);
  const actualRunId = runId ?? makeRunId(taskName);
  const run: TaskRunData = {
    runId: actualRunId,
    taskName,
    trigger,
    scheduledFor,
    startedAt: now,
    status: "running",
  };
  const docstore = yield* Docstore;
  yield* docstore.transaction("record task run start and schedule cursor", (tx) => {
    tx.upsertDoc(
      TaskRunEntity.getPk({ runId: actualRunId }),
      run,
      {
        entity: TaskRunEntity.name,
      },
      now,
    );
    tx.upsertDoc(
      TaskScheduleStateEntity.getPk({ taskName }),
      { taskName, schedule, evaluatedThrough } satisfies TaskScheduleStateData,
      { entity: TaskScheduleStateEntity.name },
      now,
    );
    const runs: TaskRunData[] = [];
    for (const row of tx.getRawRowsByPrefix(`$${TaskRunEntity.name}#`)) {
      try {
        runs.push(decodeDoc<TaskRunData>(row.data));
      } catch {
        // Entity collection reads skip corrupt rows, so pruning does too.
      }
    }
    for (const stale of selectRunsToPrune(runs, taskName, KEEP_PER_TASK)) {
      tx.deleteDoc(TaskRunEntity.getPk({ runId: stale.runId }));
      tx.deleteDoc(TaskRunLogEntity.getPk({ runId: stale.runId }));
    }
    return run;
  });
  return run;
});

export const recordRunEnd = Effect.fn("TaskRuns.recordEnd")(function* (
  runId: string,
  result: { status: "success" | "error"; error?: string; summary?: string },
  finishedAt?: number,
) {
  const now = finishedAt ?? (yield* Clock.currentTimeMillis);
  yield* TaskRunEntity.patch(
    { runId },
    {
      status: result.status,
      error: result.error,
      summary: result.summary,
      finishedAt: now,
    },
  );
});

/** Flip runs left in "running" by a crashed process to errors. Call at boot. */
export const markInterruptedRuns = Effect.fn("TaskRuns.markInterrupted")(function* () {
  const interrupted = (yield* TaskRunEntity.getAll()).filter(
    (r) => r.status === "running",
  );
  const now = yield* Clock.currentTimeMillis;
  for (const run of interrupted) {
    yield* TaskRunEntity.patch(
      { runId: run.runId },
      {
        status: "error",
        error: "interrupted (process exited)",
        finishedAt: now,
      },
    );
  }
  return interrupted.length;
});

export const getRuns = Effect.fn("TaskRuns.getRuns")(function* (
  taskName?: string,
  limit = 50,
) {
  const all = (yield* TaskRunEntity.getAll())
    .filter((r) => !taskName || r.taskName === taskName)
    .sort((a, b) => b.startedAt - a.startedAt);
  return all.slice(0, limit);
});

export const getLastRun = Effect.fn("TaskRuns.getLastRun")(function* (
  taskName: string,
) {
  return (yield* getRuns(taskName, 1))[0];
});

export const getRun = Effect.fn("TaskRuns.getRun")(function* (runId: string) {
  return Option.getOrUndefined(yield* TaskRunEntity.get({ runId }));
});

/** gzip(JSON.stringify(lines)) as base64, the stored form of captured logs. */
export function compressLogLines(lines: TaskRunLogLine[]): string {
  return gzipSync(JSON.stringify(lines)).toString("base64");
}

export function decompressLogLines(linesGz: string): TaskRunLogLine[] {
  return JSON.parse(
    gunzipSync(Buffer.from(linesGz, "base64")).toString("utf8"),
  ) as TaskRunLogLine[];
}

export const saveRunLogs = Effect.fn("TaskRuns.saveLogs")(function* (
  data: TaskRunLogData,
) {
  if (data.lines.length === 0 && data.dropped === 0) return;
  yield* TaskRunLogEntity.upsert({
    runId: data.runId,
    taskName: data.taskName,
    linesGz: compressLogLines(data.lines),
    dropped: data.dropped,
  });
});

export const getRunLogs = Effect.fn("TaskRuns.getLogs")(function* (
  runId: string,
  logger: NamedLogger,
) {
  return yield* TaskRunLogEntity.get({ runId }).pipe(
    Effect.flatMap((maybeRow) => {
      const row = Option.getOrUndefined(maybeRow);
      if (!row) return Effect.succeed(undefined);
      return Effect.try({
        try: () => {
          const lines = row.linesGz
            ? decompressLogLines(row.linesGz)
            : (row.lines ?? []);
          return {
            runId: row.runId,
            taskName: row.taskName,
            lines,
            dropped: row.dropped,
          };
        },
        catch: (cause) => cause,
      });
    }),
    Effect.catch((error) =>
      // A truncated/corrupt CBOR blob (e.g. a row half-written when the
      // container was killed mid-deploy) would otherwise 500 the logs endpoint
      // forever. Drop the unreadable row and treat the run as having no logs.
      TaskRunLogEntity.delete({ runId }).pipe(
        Effect.andThen(
          logger.warn(
            `Dropped unreadable log row for run "${runId}": ${String(error)}`,
          ),
        ),
        Effect.as(undefined),
      ),
    ),
  );
});

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

const pruneRuns = Effect.fn("TaskRuns.prune")(function* (taskName: string) {
  const stale = selectRunsToPrune(
    yield* TaskRunEntity.getAll(),
    taskName,
    KEEP_PER_TASK,
  );
  for (const run of stale) {
    yield* TaskRunEntity.delete({ runId: run.runId });
    yield* TaskRunLogEntity.delete({ runId: run.runId });
  }
});
