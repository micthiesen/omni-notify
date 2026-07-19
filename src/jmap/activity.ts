import { Entity } from "@micthiesen/mitools/entities";
import { EmailActivityLogEntity } from "./activityLogs.js";
import type { FetchedEmail } from "./emailFetcher.js";

export type EmailPipelineName = "ParcelTracker" | "CalendarEvents";

/**
 * Which tier of the filter admitted a candidate email, replacing the old
 * "sniff the reason string for `triage:`" approach. Canonical definition
 * shared by both pipelines' `filter/keywords.ts` (imported from there rather
 * than redefined) so `EmailActivityData` and the filters never drift.
 * `"carrier-name"` is parcel-only (its keyword-fallback path also checks for
 * a bare carrier name mention); calendar-events simply never produces it.
 */
export type AdmitTier =
  | "rule"
  | "builtin"
  | "triage"
  | "keyword-fallback"
  | "carrier-name";

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
  /** Structured counterpart to `admitReason` — which filter tier admitted it. */
  admitTier?: AdmitTier;
  /**
   * Total LLM cost attributed to this row in USD cents (triage, if this
   * row's admitTier is "triage", plus this pipeline's extraction call, if
   * extraction ran). `undefined` means no LLM call is attributable (a cheap
   * rule/builtin tier admitted it, or it was filtered out before triage);
   * `null` means at least one attributable call ran but its model has no
   * pricing entry, so a total can't be computed. See `sumCostCents`.
   */
  costCents?: number | null;
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
  admitTier?: AdmitTier;
  costCents?: number | null;
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
    admitTier: entry.admitTier,
    costCents: entry.costCents,
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

/**
 * Combine per-call LLM costs into one activity row's `costCents`. `undefined`
 * entries (that call never happened for this row, e.g. extraction was never
 * reached, or admitTier wasn't "triage") are dropped; if every part is
 * `undefined` the total is `undefined` too (no LLM ran at all for the row).
 * Any `null` part (a call ran on an unpriced model) makes the whole total
 * `null` rather than silently under-reporting a partially-priced row.
 */
export function sumCostCents(
  parts: Array<number | null | undefined>,
): number | null | undefined {
  const known = parts.filter((p): p is number | null => p !== undefined);
  if (known.length === 0) return undefined;
  if (known.some((p) => p === null)) return null;
  return known.reduce<number>((sum, p) => sum + (p ?? 0), 0);
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
