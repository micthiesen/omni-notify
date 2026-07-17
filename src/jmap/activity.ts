import { Entity } from "@micthiesen/mitools/entities";
import { EmailActivityLogEntity } from "./activityLogs.js";
import type { FetchedEmail } from "./emailFetcher.js";

export type EmailPipelineName = "ParcelTracker" | "CalendarEvents";

export type EmailActivityOutcome =
  /** Did not pass the candidate filter. */
  | "filtered"
  /** Passed the filter but was dropped before extraction (e.g. dedup). */
  | "skipped"
  /** Extraction ran and found nothing actionable. */
  | "no_matches"
  /** Extraction found items; see `items` for per-item results. */
  | "processed"
  /** Processing failed. */
  | "error";

export type EmailActivityData = {
  /** `${pipeline}#${emailId}` — reprocessing the same email overwrites. */
  activityId: string;
  pipeline: EmailPipelineName;
  emailId: string;
  subject: string;
  from: string;
  receivedAt: number;
  processedAt: number;
  outcome: EmailActivityOutcome;
  /** Filter reason, error message, or other context. */
  detail?: string;
  /** Short per-item results, e.g. "1Z999AA1 (ups): submitted". */
  items?: string[];
};

export const KEEP_PER_PIPELINE = 200;

export const EmailActivityEntity = new Entity<EmailActivityData, ["activityId"]>(
  "email-activity",
  ["activityId"],
);

/** Pure: rows beyond the newest `keep` for one pipeline. */
export function selectActivityToPrune(
  all: EmailActivityData[],
  pipeline: EmailPipelineName,
  keep: number,
): EmailActivityData[] {
  return all
    .filter((a) => a.pipeline === pipeline)
    .sort((a, b) => b.processedAt - a.processedAt)
    .slice(keep);
}

export function recordEmailActivity(entry: {
  pipeline: EmailPipelineName;
  email: Pick<FetchedEmail, "id" | "subject" | "from" | "receivedAt">;
  outcome: EmailActivityOutcome;
  detail?: string;
  items?: string[];
}): void {
  const receivedAt = Date.parse(entry.email.receivedAt);
  EmailActivityEntity.upsert({
    activityId: `${entry.pipeline}#${entry.email.id}`,
    pipeline: entry.pipeline,
    emailId: entry.email.id,
    subject: entry.email.subject,
    from: entry.email.from,
    receivedAt: Number.isNaN(receivedAt) ? Date.now() : receivedAt,
    processedAt: Date.now(),
    outcome: entry.outcome,
    detail: entry.detail,
    items: entry.items,
  });
  for (const stale of selectActivityToPrune(
    EmailActivityEntity.getAll(),
    entry.pipeline,
    KEEP_PER_PIPELINE,
  )) {
    EmailActivityEntity.delete({ activityId: stale.activityId });
    EmailActivityLogEntity.delete({ activityId: stale.activityId });
  }
}

export function getEmailActivity(activityId: string): EmailActivityData | undefined {
  return EmailActivityEntity.get({ activityId });
}

export function getRecentEmailActivity(
  pipeline?: EmailPipelineName,
  limit = 100,
): EmailActivityData[] {
  return EmailActivityEntity.getAll()
    .filter((a) => pipeline === undefined || a.pipeline === pipeline)
    .sort((a, b) => b.processedAt - a.processedAt)
    .slice(0, limit);
}
