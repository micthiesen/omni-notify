import { Entity } from "@micthiesen/mitools/entities";
import { Effect } from "effect";
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

export const recordEmailFeedback = Effect.fn("EmailFeedback.record")(function* (entry: {
  pipeline: EmailPipelineName;
  emailId: string;
  subject: string;
  from: string;
  verdict: EmailFeedbackVerdict;
  note?: string;
}) {
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
  yield* EmailFeedbackEntity.upsert(row);
  return row;
});

export const deleteEmailFeedback = Effect.fn("EmailFeedback.delete")(function* (
  activityId: string,
) {
  return yield* EmailFeedbackEntity.delete({ activityId });
});

export const listEmailFeedback = Effect.fn("EmailFeedback.list")(function* (
  pipeline?: EmailPipelineName,
  limit = 50,
) {
  return (yield* EmailFeedbackEntity.getAll())
    .filter((f) => pipeline === undefined || f.pipeline === pipeline)
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, limit);
});

const PIPELINE_NAMES: Record<"parcel" | "calendar", EmailPipelineName> = {
  parcel: "ParcelTracker",
  calendar: "CalendarEvents",
};

/**
 * Compact correction lines for prompt injection. Empty string when the user
 * has given no feedback for the pipeline.
 */
export const formatFeedbackDigest = Effect.fn("EmailFeedback.formatDigest")(function* (
  pipeline: "parcel" | "calendar",
  limit = 15,
) {
  return (yield* listEmailFeedback(PIPELINE_NAMES[pipeline], limit))
    .map((f) => {
      const label =
        f.verdict === "not_relevant"
          ? "user marked NOT relevant"
          : "user marked as MISSED (should have been processed)";
      const note = f.note ? ` (note: ${f.note})` : "";
      return `- "${f.subject}" from ${f.from}: ${label}${note}`;
    })
    .join("\n");
});
