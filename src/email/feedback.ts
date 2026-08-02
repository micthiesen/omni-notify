import { Entity } from "@micthiesen/mitools/entities";
import type { EmailPipelineName } from "./activity.js";

export type EmailFeedbackVerdict =
  /** The pipeline processed this email but shouldn't have. */
  | "not_relevant"
  /** The pipeline filtered this email but should have processed it. */
  | "missed";

export type EmailFeedbackData = {
  /** `${pipeline}#${emailId}` — matches the activity row's id. */
  activityId: string;
  pipeline: EmailPipelineName;
  emailId: string;
  subject: string;
  from: string;
  verdict: EmailFeedbackVerdict;
  note?: string;
  createdAt: number;
};

export const EmailFeedbackEntity = new Entity<EmailFeedbackData, ["activityId"]>(
  "email-feedback",
  ["activityId"],
);

export function recordEmailFeedback(entry: {
  pipeline: EmailPipelineName;
  emailId: string;
  subject: string;
  from: string;
  verdict: EmailFeedbackVerdict;
  note?: string;
}): EmailFeedbackData {
  const row: EmailFeedbackData = {
    activityId: `${entry.pipeline}#${entry.emailId}`,
    pipeline: entry.pipeline,
    emailId: entry.emailId,
    subject: entry.subject,
    from: entry.from,
    verdict: entry.verdict,
    note: entry.note,
    createdAt: Date.now(),
  };
  EmailFeedbackEntity.upsert(row);
  return row;
}

export function deleteEmailFeedback(activityId: string): boolean {
  if (EmailFeedbackEntity.get({ activityId }) === undefined) return false;
  EmailFeedbackEntity.delete({ activityId });
  return true;
}

export function listEmailFeedback(
  pipeline?: EmailPipelineName,
  limit = 50,
): EmailFeedbackData[] {
  return EmailFeedbackEntity.getAll()
    .filter((f) => pipeline === undefined || f.pipeline === pipeline)
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, limit);
}

const PIPELINE_NAMES: Record<"parcel" | "calendar", EmailPipelineName> = {
  parcel: "ParcelTracker",
  calendar: "CalendarEvents",
};

/**
 * Compact correction lines for prompt injection. Empty string when the user
 * has given no feedback for the pipeline.
 */
export function formatFeedbackDigest(
  pipeline: "parcel" | "calendar",
  limit = 15,
): string {
  return listEmailFeedback(PIPELINE_NAMES[pipeline], limit)
    .map((f) => {
      const label =
        f.verdict === "not_relevant"
          ? "user marked NOT relevant"
          : "user marked as MISSED (should have been processed)";
      const note = f.note ? ` (note: ${f.note})` : "";
      return `- "${f.subject}" from ${f.from}: ${label}${note}`;
    })
    .join("\n");
}
