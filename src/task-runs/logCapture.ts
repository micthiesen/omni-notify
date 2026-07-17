import { AsyncLocalStorage } from "node:async_hooks";
import { Logger } from "@micthiesen/mitools/logging";
import { runLogBus } from "./events.js";
import { saveRunLogs, type TaskRunLogLine } from "./persistence.js";

// Generous ceilings: real runs stay far below them, and persisted rows are
// gzip-compressed, so the caps only guard against a runaway logging loop.
export const MAX_LINES_PER_RUN = 20_000;
export const MAX_LINE_LENGTH = 32_768;

interface RunLogBuffer {
  taskName: string;
  lines: TaskRunLogLine[];
  dropped: number;
}

const runContext = new AsyncLocalStorage<{ runId: string }>();
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
    buffer.lines.push(line);
    if (buffer.lines.length > MAX_LINES_PER_RUN) {
      buffer.lines.shift();
      buffer.dropped++;
    }
    runLogBus.emit({ type: "line", runId: store.runId, line });
  };
}

/** Begin buffering lines for a run. */
export function startRunLogCapture(runId: string, taskName: string): void {
  buffers.set(runId, { taskName, lines: [], dropped: 0 });
}

/**
 * Execute fn with logs attributed to runId. AsyncLocalStorage carries the
 * attribution across awaits and into sub-loggers, and keeps concurrent runs
 * of different tasks separate.
 */
export function runWithLogCapture<T>(runId: string, fn: () => Promise<T>): Promise<T> {
  return runContext.run({ runId }, fn);
}

/** Live buffer contents for an in-flight run, if any. */
export function getActiveRunLogs(
  runId: string,
): { lines: TaskRunLogLine[]; dropped: number } | undefined {
  return buffers.get(runId);
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
  return buffer ? { lines: buffer.lines, dropped: buffer.dropped } : undefined;
}

/**
 * Persist the run's buffer and tell streaming clients the run is over. Must
 * be called after the run's final status is recorded, so "end" subscribers
 * read a settled run.
 */
export function finishRunLogCapture(runId: string): void {
  const buffer = buffers.get(runId);
  buffers.delete(runId);
  if (buffer) {
    saveRunLogs({
      runId,
      taskName: buffer.taskName,
      lines: buffer.lines,
      dropped: buffer.dropped,
    });
  }
  runLogBus.emit({ type: "end", runId });
}
