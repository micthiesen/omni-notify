import { Entity } from "@micthiesen/mitools/entities";
import { EmailActivityLogEntity } from "./activityLogs.js";
import type { FetchedEmail } from "./emailFetcher.js";

export type EmailPipelineName = "ParcelTracker" | "CalendarEvents";

export type EmailActivityOutcome =
  /** Did not pass the candidate filter. */
  | "filtered"
  /** Legacy only: the old whole-email dedup pre-skip emitted this; dedup is
   * now per-delivery, so new rows never use it. Kept for stored rows. */
  | "skipped"
  /** Extraction ran and found nothing actionable. */
  | "no_matches"
  /** Extraction found items and every item succeeded. */
  | "processed"
  /** Extraction found items; some succeeded and some failed. */
  | "partial"
  /** Extraction found items and every item failed (e.g. all rejected). */
  | "failed"
  /** Processing threw. */
  | "error";

/** Derive the outcome from per-item success flags (empty → no_matches). */
export function deriveItemsOutcome(itemsOk: boolean[]): EmailActivityOutcome {
  if (itemsOk.length === 0) return "no_matches";
  const succeeded = itemsOk.filter(Boolean).length;
  if (succeeded === itemsOk.length) return "processed";
  return succeeded === 0 ? "failed" : "partial";
}

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
  /** Why the email was admitted past the filter (tier/keyword/triage verdict). */
  admitReason?: string;
  /** Short per-item results, e.g. "1Z999AA1 (ups): submitted". */
  items?: string[];
};

export const KEEP_PER_PIPELINE = 1000;

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
  admitReason?: string;
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
    admitReason: entry.admitReason,
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
