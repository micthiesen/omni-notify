import type { Effect as EffectType } from "effect/Effect";
import { AsyncLocalStorage } from "node:async_hooks";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect, Exit, Fiber } from "effect";
import { runLogBus } from "./events.js";
import { saveRunLogs, type TaskRunLogLine } from "./persistence.js";

// Generous ceilings: real runs stay far below them, and persisted rows are
// gzip-compressed, so the caps only guard against a runaway logging loop.
export const MAX_LINES_PER_RUN = 20_000;
export const MAX_LINE_LENGTH = 32_768;

interface RunLogBuffer {
  taskName: string;
  lines: TaskRunLogLine[];
  nextLine: number;
  dropped: number;
}

export interface RunContext {
  runId: string;
  taskName: string;
}

const runContext = new AsyncLocalStorage<RunContext>();
const buffers = new Map<string, RunLogBuffer>();

/**
 * Route every Logger call made inside a task run to that run's log buffer.
 * Console output is untouched (and still respects LOG_LEVEL); the tap sees
 * everything down to DEBUG, so the UI can show more detail than the compose
 * logs. Lines logged outside any run (server, JMAP pipelines) are ignored.
 * Call once at boot.
 */
export function installLogCapture(): void {
  Logger.onLog = (item) => {
    const store = runContext.getStore();
    if (!store) return;
    const buffer = buffers.get(store.runId);
    if (!buffer) return;
    const text = item.formattedArgs
      ? `${item.message} ${item.formattedArgs}`
      : item.message;
    const line: TaskRunLogLine = {
      t: item.timestamp,
      level: item.level,
      logger: item.loggerName,
      msg: text.length > MAX_LINE_LENGTH ? `${text.slice(0, MAX_LINE_LENGTH)}…` : text,
    };
    if (buffer.lines.length < MAX_LINES_PER_RUN) {
      buffer.lines.push(line);
    } else {
      buffer.lines[buffer.nextLine] = line;
      buffer.nextLine = (buffer.nextLine + 1) % MAX_LINES_PER_RUN;
      buffer.dropped++;
    }
    runLogBus.emit({ type: "line", runId: store.runId, line });
  };
}

/** Begin buffering lines for a run. */
export function startRunLogCapture(runId: string, taskName: string): void {
  buffers.set(runId, { taskName, lines: [], nextLine: 0, dropped: 0 });
}

function orderedLines(buffer: RunLogBuffer): TaskRunLogLine[] {
  if (buffer.lines.length < MAX_LINES_PER_RUN || buffer.nextLine === 0) {
    return [...buffer.lines];
  }
  return [
    ...buffer.lines.slice(buffer.nextLine),
    ...buffer.lines.slice(0, buffer.nextLine),
  ];
}

/**
 * Execute fn with logs attributed to runId. AsyncLocalStorage carries the
 * attribution across awaits and into sub-loggers, and keeps concurrent runs
 * of different tasks separate.
 */
export function runWithLogCapture<T>(runId: string, fn: () => Promise<T>): Promise<T> {
  const taskName = buffers.get(runId)?.taskName ?? "Unknown";
  return runContext.run({ runId, taskName }, fn);
}

/**
 * Run an Effect inside the same AsyncLocalStorage context used by the legacy
 * Promise task runner. Effect.runForkWith is intentionally confined to this
 * adapter because establishing an ALS scope requires starting the fiber while
 * `AsyncLocalStorage.run` is active.
 */
export function runWithLogCaptureEffect<A, E, R>(
  runId: string,
  effect: EffectType<A, E, R>,
): EffectType<A, E, R> {
  return Effect.context<R>().pipe(
    Effect.flatMap((services) =>
      Effect.callback<A, E>((resume) => {
        const taskName = buffers.get(runId)?.taskName ?? "Unknown";
        const fiber = runContext.run({ runId, taskName }, () =>
          Effect.runForkWith(services)(effect),
        );
        fiber.addObserver((exit) =>
          resume(
            Exit.isSuccess(exit)
              ? Effect.succeed(exit.value)
              : Effect.failCause(exit.cause),
          ),
        );
        return Fiber.interrupt(fiber).pipe(Effect.asVoid);
      }),
    ),
  );
}

/** The runId of the task run this code is executing inside, if any. */
export function getCurrentRunId(): string | undefined {
  return runContext.getStore()?.runId;
}

/** Current task or synthetic email-pipeline capture, used for cost attribution. */
export function getCurrentRunContext(): RunContext | undefined {
  return runContext.getStore();
}

/** Live buffer contents for an in-flight run, if any. */
export function getActiveRunLogs(
  runId: string,
): { lines: TaskRunLogLine[]; dropped: number } | undefined {
  const buffer = buffers.get(runId);
  return buffer ? { lines: orderedLines(buffer), dropped: buffer.dropped } : undefined;
}

/**
 * Stop buffering for an ad-hoc capture (e.g. per-email pipeline work) and
 * return the collected lines instead of persisting them as a task run.
 */
export function takeRunLogCapture(
  id: string,
): { lines: TaskRunLogLine[]; dropped: number } | undefined {
  const buffer = buffers.get(id);
  buffers.delete(id);
  return buffer ? { lines: orderedLines(buffer), dropped: buffer.dropped } : undefined;
}

/**
 * Persist the run's buffer and tell streaming clients the run is over. Must
 * be called after the run's final status is recorded, so "end" subscribers
 * read a settled run.
 */
export function finishRunLogCapture(runId: string): void {
  const buffer = buffers.get(runId);
  buffers.delete(runId);
  try {
    if (buffer) {
      saveRunLogs({
        runId,
        taskName: buffer.taskName,
        lines: orderedLines(buffer),
        dropped: buffer.dropped,
      });
    }
  } finally {
    runLogBus.emit({ type: "end", runId });
  }
}
