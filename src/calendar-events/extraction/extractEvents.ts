import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import { getExtractionModel } from "../../ai/registry.js";
import { codeBlock } from "../../utils/markdown.js";
import {
  type CalendarEventExtraction,
  calendarEventExtractionSchema,
} from "./schema.js";

const MAX_BODY_CHARS = 3000;

export async function extractCalendarEvents(
  email: { subject: string; from: string; textBody: string },
  logger: Logger,
  logFile?: LogFile,
): Promise<CalendarEventExtraction["events"]> {
  const { model, modelId } = getExtractionModel();
  const body = email.textBody.slice(0, MAX_BODY_CHARS);

  const currentDate = new Date().toLocaleDateString("en-US", {
    weekday: "long",
    year: "numeric",
    month: "long",
    day: "numeric",
  });

  const prompt = `Extract calendar events from this email that the recipient would want on their personal calendar. Return an empty events array if no actionable events are found.

Guidelines:
- Only extract real, scheduled events: appointments, flights, hotel stays, concert tickets, reservations, meetings, etc.
- Do NOT extract: sale deadlines, marketing urgency ("offer expires"), subscription renewals, shipping delivery windows, password expiration warnings, generic reminders without specific dates
- For flights: create one event per flight segment (outbound, return, connections)
- For hotel stays: create one event spanning check-in to check-out
- For appointments: use the appointment time, not the "arrive by" time
- Infer timezone from location context when not explicitly stated (e.g. JFK airport → America/New_York, a restaurant in London → Europe/London, a hotel in Tokyo → Asia/Tokyo). Only leave timeZone empty if there are no geographic clues at all
- If only a date is mentioned with no time, set allDay to true
- Prefer explicit information over inference. If the email doesn't clearly state when something happens, don't guess
- Title should be concise and descriptive in Title Case (e.g. "Dentist Appointment", "Flight YYZ → YVR", "Hamilton at Princess of Wales Theatre")

Today's date: ${currentDate}

From: ${email.from}
Subject: ${email.subject}

${body}`;

  if (logFile) {
    logFile.log(
      logger,
      LogLevel.INFO,
      `Extraction Prompt (${modelId})`,
      codeBlock(prompt),
      { consoleSummary: `Extraction prompt (${modelId}) [${prompt.length} chars]` },
    );
  } else {
    logger.info(`Extraction prompt (${modelId}):\n${prompt}`);
  }

  const result = await generateText({
    model,
    output: Output.object({ schema: calendarEventExtractionSchema }),
    prompt,
  });

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

  return result.output?.events ?? [];
}
