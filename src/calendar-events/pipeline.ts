import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import type { JmapContext } from "../jmap/client.js";
import type { EmailAttachment } from "../jmap/emailFetcher.js";
import { fetchNewEmails } from "../jmap/emailFetcher.js";
import config from "../utils/config.js";
import { logTimestamp } from "../utils/markdown.js";
import { downloadSupportedAttachments } from "./extraction/attachments.js";
import { extractCalendarEvents } from "./extraction/extractEvents.js";
import { createCalendarEvent, discoverCalendarUrl } from "./fastmail/calendarApi.js";
import { isCalendarCandidate } from "./filter/keywords.js";
import {
  computeEventHash,
  getCalendarEmailState,
  hasCreatedEvent,
  recordCreatedEvent,
  saveCalendarEmailState,
} from "./persistence.js";

export class CalendarEventPipeline {
  private logger: Logger;
  private ctx: JmapContext;
  private processing = false;
  private calendarUrl?: string;

  constructor(ctx: JmapContext, logger: Logger) {
    this.ctx = ctx;
    this.logger = logger;
  }

  async onEmailStateChange(): Promise<void> {
    if (this.processing) {
      this.logger.debug("Pipeline already processing, skipping");
      return;
    }

    this.processing = true;
    try {
      await this.processStateChange();
    } finally {
      this.processing = false;
    }
  }

  private async processStateChange(): Promise<void> {
    const sinceState = getCalendarEmailState();

    if (!sinceState) {
      this.logger.info("First run: fetching current JMAP state (skipping history)");
      const state = await this.fetchCurrentEmailState();
      if (state) {
        saveCalendarEmailState(state);
        this.logger.info(`Saved initial JMAP state: ${state}`);
      }
      return;
    }

    let emails: Awaited<ReturnType<typeof fetchNewEmails>>["emails"];
    let newState: string;
    try {
      const result = await fetchNewEmails(this.ctx, sinceState, this.logger);
      emails = result.emails;
      newState = result.newState;
    } catch (error) {
      const message = (error as Error).message ?? "";
      if (message.includes("cannotCalculateChanges")) {
        this.logger.warn("cannotCalculateChanges: resetting state");
        const state = await this.fetchCurrentEmailState();
        if (state) saveCalendarEmailState(state);
        return;
      }
      this.logger.error("Failed to fetch emails", message);
      return;
    }

    // Filter candidates
    const candidates = [];
    for (const email of emails) {
      const candidate = isCalendarCandidate(
        { from: email.from, subject: email.subject, textBody: email.textBody },
        this.logger,
      );
      if (candidate) {
        candidates.push(email);
      } else {
        this.logger.info(`Filtered out: "${email.subject}" from ${email.from}`);
      }
    }

    if (candidates.length > 0) {
      this.logger.info(
        `${candidates.length} calendar candidate(s) from ${emails.length} new email(s)`,
      );
    } else if (emails.length > 0) {
      this.logger.info(`No calendar candidates in ${emails.length} new email(s)`);
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
        saveCalendarEmailState(newState);
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

    saveCalendarEmailState(newState);
  }

  private async processEmail(
    email: {
      id: string;
      subject: string;
      from: string;
      textBody: string;
      attachments: EmailAttachment[];
    },
    runLog?: LogFile,
  ): Promise<void> {
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

  private async fetchCurrentEmailState(): Promise<string | undefined> {
    const [result] = await this.ctx.jam.request([
      "Email/get",
      { accountId: this.ctx.accountId, ids: [] },
    ]);
    return (result as Record<string, unknown>).state as string | undefined;
  }
}
