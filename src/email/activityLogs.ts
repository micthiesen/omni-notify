import { Entity } from "@micthiesen/mitools/entities";
import { Logger } from "@micthiesen/mitools/logging";
import { Data, Effect } from "effect";
import {
  runWithLogCaptureEffect,
  startRunLogCapture,
  takeRunLogCapture,
} from "../task-runs/logCapture.js";
import {
  compressLogLines,
  decompressLogLines,
  type TaskRunLogLine,
} from "../task-runs/persistence.js";

const logger = new Logger("EmailActivityLogs");

export type EmailActivityLogData = {
  activityId: string;
  lines: TaskRunLogLine[];
  /** Oldest lines dropped once the per-capture cap was hit. */
  dropped: number;
};

type StoredEmailActivityLog = {
  activityId: string;
  /** gzip(JSON.stringify(lines)), base64-encoded. */
  linesGz: string;
  dropped: number;
};

/**
 * One row per email that reached a pipeline's processing phase (extraction and
 * onward). Overwritten on reprocess; pruned alongside the activity rows.
 */
export const EmailActivityLogEntity = new Entity<
  StoredEmailActivityLog,
  ["activityId"]
>("email-activity-log", ["activityId"]);

export class EmailLogCaptureError extends Data.TaggedError("EmailLogCaptureError")<{
  readonly activityId: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return this.cause instanceof Error ? this.cause.message : String(this.cause);
  }
}

/**
 * Run fn with every log line attributed to this email's activity record, then
 * persist the capture. Filter-phase skips never enter here, so a missing log
 * row simply means the email never reached processing.
 */
export function withEmailLogCaptureEffect<T, E>(
  activityId: string,
  pipeline: string,
  effect: () => Effect.Effect<T, E, never>,
): Effect.Effect<T, E> {
  return Effect.uninterruptibleMask((restore) =>
    Effect.gen(function* () {
      startRunLogCapture(activityId, pipeline);
      const result = yield* Effect.exit(
        restore(runWithLogCaptureEffect(activityId, effect())),
      );
      yield* Effect.try({
        try: () => {
          const buffer = takeRunLogCapture(activityId);
          if (buffer) {
            saveEmailActivityLogs({
              activityId,
              lines: buffer.lines,
              dropped: buffer.dropped,
            });
          }
        },
        catch: (cause) => new EmailLogCaptureError({ activityId, cause }),
      }).pipe(
        Effect.catch((error) =>
          Effect.sync(() =>
            logger.warn(
              `Could not persist diagnostic log for "${activityId}": ${error.message}`,
            ),
          ).pipe(Effect.catchCause(() => Effect.void)),
        ),
      );
      return yield* result;
    }),
  );
}

export function saveEmailActivityLogs(data: EmailActivityLogData): void {
  if (data.lines.length === 0 && data.dropped === 0) {
    EmailActivityLogEntity.delete({ activityId: data.activityId });
    return;
  }
  EmailActivityLogEntity.upsert({
    activityId: data.activityId,
    linesGz: compressLogLines(data.lines),
    dropped: data.dropped,
  });
}

export function getEmailActivityLogs(
  activityId: string,
): EmailActivityLogData | undefined {
  try {
    const row = EmailActivityLogEntity.get({ activityId });
    if (!row) return undefined;
    return {
      activityId,
      lines: decompressLogLines(row.linesGz),
      dropped: row.dropped,
    };
  } catch (err) {
    // Drop an unreadable row rather than 500 the logs endpoint forever.
    EmailActivityLogEntity.delete({ activityId });
    logger.warn(`Dropped unreadable log row for "${activityId}": ${String(err)}`);
    return undefined;
  }
}
