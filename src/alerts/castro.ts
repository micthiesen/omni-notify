import type { LogHook } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { getRuns, type TaskRunData } from "../task-runs/persistence.js";

const TASK = "CastroInboxCleanup";
const MIN_FAILURE_SPAN_MS = 12 * 60 * 60_000;
// Allow cron jitter around the twelve-hour boundary.
const JITTER_MS = 5 * 60_000;

/** History is newest first; a successful run ends the incident, including after reboot. */
export function hasPersistentCastroFailure(runs: readonly TaskRunData[]): boolean {
  const failures: TaskRunData[] = [];
  for (const run of runs) {
    if (run.status !== "error") break;
    failures.push(run);
  }
  return (
    failures.length >= 3 &&
    failures[0].startedAt - failures[failures.length - 1].startedAt >=
      MIN_FAILURE_SPAN_MS - JITTER_MS
  );
}

/** Gate only cleanup notifications; the failed runs and error logs stay intact. */
export function persistentCastroFailureHook<R>(inner: LogHook<R>) {
  const titles = new Set([
    `Error running task "${TASK}"`,
    `Manual run of "${TASK}" failed`,
    `Catch-up run of "${TASK}" failed`,
  ]);
  const hook: LogHook<R | Effect.Services<ReturnType<typeof getRuns>>> = (
    notification,
  ) => {
    if (!titles.has(notification.title)) return inner(notification);
    return getRuns(TASK).pipe(
      Effect.flatMap((runs) =>
        hasPersistentCastroFailure(runs) ? inner(notification) : Effect.void,
      ),
      // Missing history cannot establish a persistent Castro failure.
      Effect.catch(() => Effect.void),
    );
  };
  return hook;
}
