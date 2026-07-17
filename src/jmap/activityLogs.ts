import { Entity } from "@micthiesen/mitools/entities";
import { Logger } from "@micthiesen/mitools/logging";
import {
  runWithLogCapture,
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

/**
 * Run fn with every log line attributed to this email's activity record, then
 * persist the capture. Filter-phase skips never enter here, so a missing log
 * row simply means the email never reached processing.
 */
export async function withEmailLogCapture<T>(
  activityId: string,
  pipeline: string,
  fn: () => Promise<T>,
): Promise<T> {
  startRunLogCapture(activityId, pipeline);
  try {
    return await runWithLogCapture(activityId, fn);
  } finally {
    const buffer = takeRunLogCapture(activityId);
    if (buffer) {
      saveEmailActivityLogs({
        activityId,
        lines: buffer.lines,
        dropped: buffer.dropped,
      });
    }
  }
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
