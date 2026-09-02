import { Entity } from "@micthiesen/mitools/entities";
import { Logger } from "@micthiesen/mitools/logging";
import { randomUUID } from "node:crypto";
import { Data, Effect, Option } from "effect";
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

const logger = Logger.named("EmailActivityLogs");

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
export function withEmailLogCaptureEffect<T, E, R>(
  activityId: string,
  pipeline: string,
  effect: () => Effect.Effect<T, E, R>,
) {
  return Effect.uninterruptibleMask((restore) =>
    Effect.gen(function* () {
      const captureId = `${activityId}:${randomUUID()}`;
      startRunLogCapture(captureId, pipeline);
      const result = yield* Effect.exit(
        restore(runWithLogCaptureEffect(captureId, effect())),
      );
      const buffer = takeRunLogCapture(captureId);
      if (buffer) {
        yield* saveEmailActivityLogs({
          activityId,
          lines: buffer.lines,
          dropped: buffer.dropped,
        }).pipe(
          Effect.mapError((cause) => new EmailLogCaptureError({ activityId, cause })),
          Effect.catch((error) =>
            logger.warn(
              `Could not persist diagnostic log for "${activityId}": ${error.message}`,
            ),
          ),
        );
      }
      return yield* result;
    }),
  );
}

export const saveEmailActivityLogs = Effect.fn("EmailActivityLogs.save")(function* (
  data: EmailActivityLogData,
) {
  if (data.lines.length === 0 && data.dropped === 0) {
    yield* EmailActivityLogEntity.delete({ activityId: data.activityId });
    return;
  }
  yield* EmailActivityLogEntity.upsert({
    activityId: data.activityId,
    linesGz: compressLogLines(data.lines),
    dropped: data.dropped,
  });
});

export const getEmailActivityLogs = Effect.fn("EmailActivityLogs.get")(function* (
  activityId: string,
) {
  return yield* Effect.gen(function* () {
    const row = Option.getOrUndefined(
      yield* EmailActivityLogEntity.get({ activityId }),
    );
    if (!row) return undefined;
    return {
      activityId,
      lines: decompressLogLines(row.linesGz),
      dropped: row.dropped,
    };
  }).pipe(
    Effect.catch((err) =>
      Effect.gen(function* () {
        // Drop an unreadable row rather than 500 the logs endpoint forever.
        yield* EmailActivityLogEntity.delete({ activityId });
        yield* logger.warn(
          `Dropped unreadable log row for "${activityId}": ${String(err)}`,
        );
        return undefined;
      }),
    ),
  );
});
