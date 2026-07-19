import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output, type UserContent } from "ai";
import { hasPrice, llmCostCents } from "../../ai/cost.js";
import { getCalendarExtractionModel } from "../../ai/registry.js";
import type { DownloadedAttachment } from "./attachments.js";
import {
  isDegenerateExtraction,
  type SanitizeResult,
  sanitizeExtractedEvents,
} from "./sanitize.js";
import {
  type CalendarEventExtraction,
  calendarEventExtractionSchema,
} from "./schema.js";

const MAX_BODY_CHARS = 12000;

export interface ExistingEventContext {
  id: string;
  title: string;
  startDate: string;
  startTime?: string;
  endDate?: string;
  endTime?: string;
  allDay: boolean;
  location?: string;
  timeZone?: string;
}

export interface ExtractCalendarEventsOptions {
  email: { subject: string; from: string; textBody: string };
  logger: Logger;
  logFile?: LogFile;
  attachments?: DownloadedAttachment[];
  localTimeZone?: string;
  existingEvents?: ExistingEventContext[];
}

export interface ExtractCalendarEventsResult {
  events: CalendarEventExtraction["events"];
  /**
   * USD cents across every generateText call this run made (the degenerate-
   * output retry is a second real call, so its cost counts too, regardless
   * of whether the retry's events end up used). Null if any contributing
   * call ran on an unpriced model.
   */
  costCents: number | null;
}

/** One `generateText` attempt's sanitized output plus its cost. */
interface ExtractionAttempt extends SanitizeResult {
  costCents: number | null;
}

/** Sum two attempt costs; null propagates (an unpriced call taints the total). */
function addCostCents(a: number | null, b: number | null): number | null {
  return a === null || b === null ? null : a + b;
}

function formatExistingEvent(e: ExistingEventContext): string {
  const parts = [`- [${e.id}] "${e.title}" on ${e.startDate}`];
  if (e.allDay) {
    parts.push("(all day)");
  } else if (e.startTime) {
    parts.push(e.endTime ? `${e.startTime}–${e.endTime}` : `at ${e.startTime}`);
  }
  if (e.timeZone) parts.push(`(${e.timeZone})`);
  if (e.location) parts.push(`@ ${e.location}`);
  return parts.join(" ");
}

export async function extractCalendarEvents(
  options: ExtractCalendarEventsOptions,
): Promise<ExtractCalendarEventsResult> {
  const { email, logger, logFile, attachments, localTimeZone, existingEvents } =
    options;
  const { model, modelId } = getCalendarExtractionModel();
  const body = email.textBody.slice(0, MAX_BODY_CHARS);

  const currentDate = new Date().toLocaleDateString("en-US", {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
    timeZone: localTimeZone,
  });

  const promptText = `Extract calendar events from this email that the recipient would want on their personal calendar. Return an empty events array if no actionable events are found.

Guidelines:
- Extract real, scheduled events: appointments, flights, hotel stays, concert tickets, reservations, meetings, building maintenance/shutdowns, move-in/out dates, etc.
- Also extract building/strata notices (water shutdowns, power outages, maintenance windows, fire alarm tests) — these affect the recipient's schedule
- Do NOT extract: terms of service updates, privacy policy changes, or other legal/policy notices
- Do NOT extract: sale deadlines, marketing urgency ("offer expires"), password expiration warnings
- Do NOT extract: package delivery or shipping notifications (these are handled by a separate parcel tracking system)
- Do NOT extract any billing, payment, or subscription event — subscription renewals, recurring or auto-pay charges (e.g. iCloud+, Netflix, a PayPal automatic-payment setup), domain/plan renewals, invoices, or payment due dates — regardless of cadence (monthly, annual, or one-off). These are auto-charged and not actionable. This exclusion is about money charged or owed; genuine action-required deadlines that are NOT about payment (e.g. securing API keys by a date, renewing a passport) should still be extracted. A receipt or confirmation for a real scheduled event (a paid concert ticket, a flight or hotel booking) IS still extractable — put the event on the calendar, just never the payment/charge itself
- For flights: create one event per flight segment (outbound, return, connections)
- For multi-day events (hotel stays, retreats, conferences): create one event spanning the first day to the last (set endDate), not separate events per day
- For notices repeating on a fixed pattern: a notice like "daily 9:00-16:00 from Jul 6 to Jul 13" is ONE event on the first day (startDate the first day, startTime 09:00, endTime 16:00) with recurrence { frequency: "daily", until: the last day }. Do NOT set endDate to the last day of the pattern and do NOT create one event per day; endDate is only for a single continuous stay spanning multiple days
- For appointments: use the appointment time, not the "arrive by" time
- Always extract endTime when a time range is given (e.g. "8:00 a.m. – 5:00 p.m." → startTime 08:00, endTime 17:00). Do not omit the end time
- Infer timezone from location context when not explicitly stated (e.g. JFK airport → America/New_York, a restaurant in London → Europe/London, a hotel in Tokyo → Asia/Tokyo)${localTimeZone ? `. When there are no geographic clues, use the recipient's local timezone: ${localTimeZone}` : ". Only leave timeZone empty if there are no geographic clues at all"}
- If only a date is mentioned with no time, set allDay to true
- Extract events even when details are partial — include what's available (e.g. a date in the subject line with no time → allDay event)
- Look for dates in subject lines, headers, and filenames mentioned in the email, not just the body text
- If attachments are included, extract event details from them as well (PDFs, images with text)
- Title should be prefixed with a relevant emoji and be concise and descriptive in Title Case (e.g. "🦷 Dentist Appointment", "✈️ Flight YYZ → YVR", "🎭 Hamilton at Princess of Wales Theatre")
- Set reminderMinutes for events that benefit from advance preparation. Examples: flights/travel (1440 = day before), building shutoffs/maintenance (720 = night before), appointments/reservations (60 = 1 hour). Omit for events where the default 30-minute reminder is fine

Action classification:
- Use "create" for new events not already in the existing events list below. Omit eventId
- Use "cancel" if the email indicates an existing event has been cancelled, voided, or is no longer happening
- Payment receipts and bills confirm past service — they NEVER cancel an upcoming event. Never emit "cancel" because a payment, receipt, invoice, or billing email mentions an appointment or service
- A "cancel" is only honored when its eventId references an existing event from the list below; a cancel without a valid eventId is skipped
- Use "update" if the email indicates an existing event has been rescheduled, moved, or had details changed (new time, location, etc.)
- For "cancel" and "update", set eventId to the id shown in square brackets next to the existing event you are acting on, WITHOUT the brackets (for [evt_2], use evt_2). Copy it exactly. Only use an id from the list; never invent one. Keep the same title and startDate as that existing event
- If an update fundamentally changes the event (e.g. rebooked to a completely different flight), emit a "cancel" for the old event (with its eventId) and a "create" for the new one
- Do NOT generate "cancel" or "update" for events not in the existing events list
- If an email is just a reminder or confirmation for an existing event with no actual changes (same date, time, location), return an empty events array. Do NOT emit an "update" unless something has actually changed
- For updates, include ALL event fields (startTime, endTime, location, timeZone, etc.), not just the changed ones. The update replaces the entire event
- When in doubt, prefer "create"
${existingEvents && existingEvents.length > 0 ? `\nExisting calendar events (created by this system):\n${existingEvents.map((e) => formatExistingEvent(e)).join("\n")}\n` : ""}
Today's date: ${currentDate}

From: ${email.from}
Subject: ${email.subject}

${body}`;

  const attachmentNames = attachments?.map((a) => a.name).join(", ");
  const logSummary = attachmentNames
    ? `Extraction prompt (${modelId}) [${promptText.length} chars, attachments: ${attachmentNames}]`
    : `Extraction prompt (${modelId}) [${promptText.length} chars]`;

  if (logFile) {
    logFile.log(
      logger,
      LogLevel.INFO,
      `Extraction Prompt (${modelId})`,
      codeBlock(promptText),
      {
        consoleSummary: logSummary,
      },
    );
  } else {
    logger.info(logSummary);
  }

  // Build content parts: text + optional file attachments
  const content: UserContent = [{ type: "text", text: promptText }];

  if (attachments && attachments.length > 0) {
    for (const attachment of attachments) {
      content.push({
        type: "file",
        data: attachment.data,
        mediaType: attachment.mimeType,
      });
    }
    logger.info(`Including ${attachments.length} attachment(s): ${attachmentNames}`);
  }

  const first = await runExtractionOnce({ model, content, logger, logFile, modelId });

  // Degenerate outputs (mass-duplicated objects, field soup in timeZone) tend to
  // be bad samples — one fresh retry usually recovers; keep whichever is cleaner.
  if (!isDegenerateExtraction(first)) {
    return { events: first.events, costCents: first.costCents };
  }
  logger.warn(
    `Degenerate extraction output (${first.issues.join("; ")}); retrying once`,
  );
  const second = await runExtractionOnce({ model, content, logger, logFile, modelId });
  const costCents = addCostCents(first.costCents, second.costCents);
  if (second.issues.length < first.issues.length) {
    logger.info("Retry produced a cleaner extraction; using the retry result");
    return { events: second.events, costCents };
  }
  logger.info("Retry did not improve on the first extraction; keeping the first");
  return { events: first.events, costCents };
}

async function runExtractionOnce(args: {
  model: ReturnType<typeof getCalendarExtractionModel>["model"];
  content: UserContent;
  logger: Logger;
  logFile?: LogFile;
  modelId: string;
}): Promise<ExtractionAttempt> {
  const { model, content, logger, logFile, modelId } = args;

  const result = await generateText({
    model,
    output: Output.object({ schema: calendarEventExtractionSchema }),
    messages: [{ role: "user", content }],
  });

  if (result.reasoningText && logFile) {
    logFile.log(logger, LogLevel.INFO, "Reasoning", codeBlock(result.reasoningText));
  }

  const response = JSON.stringify(result.output, null, 2);
  if (logFile) {
    logFile.log(
      logger,
      LogLevel.INFO,
      "Extraction Response",
      codeBlock(response, "json"),
    );
  } else {
    logger.info(`Extraction response: ${response}`);
  }
  logger.info(
    `Token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  const costCents = hasPrice(modelId)
    ? llmCostCents(modelId, {
        inputTokens: result.usage.inputTokens ?? 0,
        outputTokens: result.usage.outputTokens ?? 0,
      })
    : null;
  if (costCents === null) {
    logger.debug(`No pricing data for extraction model "${modelId}"`);
  }

  const sanitized = sanitizeExtractedEvents(result.output?.events ?? []);
  for (const issue of sanitized.issues) {
    logger.warn(`Sanitized extraction output: ${issue}`);
  }
  return { ...sanitized, costCents };
}
