import type { Effect as EffectType } from "effect/Effect";
import { AsyncLocalStorage } from "node:async_hooks";
import type { LogTap } from "@micthiesen/mitools/logging";
import { Context, Effect, Fiber } from "effect";
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
const EffectRunContext = Context.Reference<RunContext | undefined>(
  "omni-notify/TaskRunLogContext",
  { defaultValue: () => undefined },
);
const buffers = new Map<string, RunLogBuffer>();

function currentRunContext(): RunContext | undefined {
  const fiber = Fiber.getCurrent();
  const effectContext = fiber
    ? Context.get(fiber.context, EffectRunContext)
    : undefined;
  return effectContext ?? runContext.getStore();
}

/**
 * Route every Logger call made inside a task run to that run's log buffer.
 * Console output is untouched (and still respects LOG_LEVEL); the tap sees
 * everything down to DEBUG, so the UI can show more detail than the compose
 * logs. Lines logged outside any run (server, email pipelines) are ignored.
 * Call once at boot.
 */
export const taskLogTap: LogTap = (item) =>
  Effect.sync(() => {
    const store = currentRunContext();
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
  });

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
 * Attribute an Effect with a fiber-local run context. Unlike Node
 * AsyncLocalStorage, the Effect context is retained across scheduler
 * suspensions and inherited by child fibers.
 */
export function runWithLogCaptureEffect<A, E, R>(
  runId: string,
  effect: EffectType<A, E, R>,
): EffectType<A, E, R> {
  const taskName = buffers.get(runId)?.taskName ?? "Unknown";
  return Effect.provideService(effect, EffectRunContext, { runId, taskName });
}

/** The runId of the task run this code is executing inside, if any. */
export function getCurrentRunId(): string | undefined {
  return currentRunContext()?.runId;
}

/** Current task or synthetic email-pipeline capture, used for cost attribution. */
export function getCurrentRunContext(): RunContext | undefined {
  return currentRunContext();
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
export const finishRunLogCapture = Effect.fn("TaskRunLogs.finish")(function* (
  runId: string,
) {
  const buffer = buffers.get(runId);
  buffers.delete(runId);
  try {
    if (buffer) {
      yield* saveRunLogs({
        runId,
        taskName: buffer.taskName,
        lines: orderedLines(buffer),
        dropped: buffer.dropped,
      });
    }
  } finally {
    runLogBus.emit({ type: "end", runId });
  }
});
