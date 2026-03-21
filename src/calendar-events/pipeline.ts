import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import type { JmapContext } from "../jmap/client.js";
import type { EmailHandler } from "../jmap/dispatcher.js";
import type { FetchedEmail } from "../jmap/emailFetcher.js";
import config from "../utils/config.js";
import { logTimestamp } from "../utils/markdown.js";
import { downloadSupportedAttachments } from "./extraction/attachments.js";
import { extractCalendarEvents } from "./extraction/extractEvents.js";
import { createCalendarEvent, discoverCalendarUrl } from "./fastmail/calendarApi.js";
import { filterCalendarCandidate } from "./filter/keywords.js";
import {
  computeEventHash,
  hasCreatedEvent,
  recordCreatedEvent,
} from "./persistence.js";

export class CalendarEventPipeline implements EmailHandler {
  public readonly name = "CalendarEvents";
  private logger: Logger;
  private ctx: JmapContext;
  private calendarUrl?: string;

  constructor(ctx: JmapContext, logger: Logger) {
    this.ctx = ctx;
    this.logger = logger;
  }

  async handleEmails(emails: FetchedEmail[]): Promise<void> {
    // Filter candidates
    const candidates = [];
    for (const email of emails) {
      const result = filterCalendarCandidate({
        from: email.from,
        subject: email.subject,
        textBody: email.textBody,
      });
      if (result.pass) {
        this.logger.info(
          `Candidate (${result.reason}): "${email.subject}" from ${email.from}`,
        );
        candidates.push(email);
      } else {
        this.logger.info(
          `Skipped (${result.reason}): "${email.subject}" from ${email.from}`,
        );
      }
    }

    // Create a fresh run log per batch
    const runLog = config.LOGS_PATH
      ? new LogFile(
          `${config.LOGS_PATH}/calendar-events/${logTimestamp()}.md`,
          "overwrite",
        )
      : undefined;

    // Discover calendar URL once (lazy init + cache)
    if (candidates.length > 0 && !this.calendarUrl) {
      try {
        this.calendarUrl = await discoverCalendarUrl(this.logger);
      } catch (error) {
        this.logger.error(
          "Failed to discover calendar URL, skipping batch",
          (error as Error).message,
        );
        return;
      }
    }

    // Process each candidate
    for (const email of candidates) {
      try {
        await this.processEmail(email, runLog);
      } catch (error) {
        this.logger.error(
          `Failed to process email "${email.subject}"`,
          (error as Error).message,
        );
      }
    }
  }

  private async processEmail(email: FetchedEmail, runLog?: LogFile): Promise<void> {
    this.logger.info(
      `Extracting events from: "${email.subject}" (from: ${email.from})`,
    );

    // Download supported attachments (PDFs, images)
    const downloaded = await downloadSupportedAttachments(
      this.ctx,
      email.attachments,
      this.logger,
    );

    let events: Awaited<ReturnType<typeof extractCalendarEvents>>;
    try {
      events = await extractCalendarEvents(
        { subject: email.subject, from: email.from, textBody: email.textBody },
        this.logger,
        runLog,
        downloaded.length > 0 ? downloaded : undefined,
      );
    } catch (error) {
      this.logger.error(
        `Extraction failed for "${email.subject}" from ${email.from}`,
        (error as Error).message,
      );
      return;
    }

    if (events.length === 0) {
      this.logger.info(`No calendar events found in "${email.subject}"`);
      return;
    }

    this.logger.info(`Found ${events.length} event(s) in "${email.subject}"`);

    for (const event of events) {
      try {
        await this.processEvent(event, email.id);
      } catch (error) {
        this.logger.error(
          `Failed to create event "${event.title}"`,
          (error as Error).message,
        );
      }
    }
  }

  private async processEvent(
    event: Awaited<ReturnType<typeof extractCalendarEvents>>[number],
    emailId: string,
  ): Promise<void> {
    const eventHash = computeEventHash(event.title, event.startDate, event.startTime);

    if (hasCreatedEvent(eventHash)) {
      this.logger.info(
        `Duplicate event: "${event.title}" on ${event.startDate} (skipping)`,
      );
      return;
    }

    if (!this.calendarUrl) {
      this.logger.error("Calendar URL not discovered, cannot create event");
      return;
    }

    const result = await createCalendarEvent(this.calendarUrl, event, this.logger);

    if (result.status === "error") {
      this.logger.error(
        `Failed to create calendar event "${event.title}": ${result.message}`,
      );
      return;
    }

    recordCreatedEvent({
      eventHash,
      emailId,
      calendarEventId: result.eventUid,
      title: event.title,
      startDate: event.startDate,
      createdAt: Date.now(),
    });

    // Send notification
    const dateStr = event.startDate;
    const timePart = event.allDay
      ? "(all day)"
      : event.startTime
        ? `at ${event.startTime}`
        : "";
    try {
      await notify({
        title: "Calendar Event Created",
        message: `${event.title}\n${dateStr}${timePart ? ` ${timePart}` : ""}${event.location ? `\n${event.location}` : ""}`,
        token: config.PUSHOVER_CALENDAR_TOKEN,
      });
    } catch (error) {
      this.logger.warn("Failed to send notification", (error as Error).message);
    }

    this.logger.info(
      `Created: "${event.title}" on ${dateStr}${timePart ? ` ${timePart}` : ""}`,
    );
  }
}
