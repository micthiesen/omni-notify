import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { notify } from "@micthiesen/mitools/pushover";
import type { JmapContext } from "../jmap/client.js";
import type { EmailHandler } from "../jmap/dispatcher.js";
import type { FetchedEmail } from "../jmap/emailFetcher.js";
import config from "../utils/config.js";
import { downloadSupportedAttachments } from "./extraction/attachments.js";
import {
  type ExistingEventContext,
  extractCalendarEvents,
} from "./extraction/extractEvents.js";
import type { CalendarEventExtraction } from "./extraction/schema.js";
import {
  createCalendarEvent,
  deleteCalendarEvent,
  discoverCalendarUrl,
  updateCalendarEvent,
} from "./fastmail/calendarApi.js";
import { filterCalendarCandidate } from "./filter/keywords.js";
import {
  type CreatedCalendarEventData,
  computeEventHash,
  getRecentEvents,
  hasCreatedEvent,
  hasEventChanged,
  markEventCancelled,
  recordCreatedEvent,
  resolveEventReference,
} from "./persistence.js";

type ExtractedEvent = CalendarEventExtraction["events"][number];

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
      const runLog = config.LOGS_PATH
        ? new LogFile(
            `${config.LOGS_PATH}/calendar-events/${logTimestamp()}.md`,
            "overwrite",
          )
        : undefined;
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

    // Provide existing events as context for cancel/update matching, each tagged with a
    // stable per-prompt handle (evt_N) the model echoes back to identify its target —
    // decoupling matching from the regenerated title.
    const existingById = new Map<string, CreatedCalendarEventData>();
    const existingEvents: ExistingEventContext[] = getRecentEvents().map((e, i) => {
      const id = `evt_${i + 1}`;
      existingById.set(id, e);
      return {
        id,
        title: e.title,
        startDate: e.startDate,
        startTime: e.startTime,
        endDate: e.endDate,
        endTime: e.endTime,
        allDay: e.allDay,
        location: e.location,
        timeZone: e.timeZone,
      };
    });

    let events: Awaited<ReturnType<typeof extractCalendarEvents>>;
    try {
      events = await extractCalendarEvents({
        email: { subject: email.subject, from: email.from, textBody: email.textBody },
        logger: this.logger,
        logFile: runLog,
        attachments: downloaded.length > 0 ? downloaded : undefined,
        localTimeZone: config.TZ,
        existingEvents,
      });
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
        switch (event.action) {
          case "create":
            await this.handleCreate(event, email.id);
            break;
          case "cancel":
            await this.handleCancel(event, existingById);
            break;
          case "update":
            await this.handleUpdate(event, existingById, email.id);
            break;
        }
      } catch (error) {
        this.logger.error(
          `Failed to process event "${event.title}" (${event.action})`,
          (error as Error).message,
        );
      }
    }
  }

  private async handleCreate(event: ExtractedEvent, emailId: string): Promise<void> {
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
      startTime: event.startTime,
      endDate: event.endDate,
      endTime: event.endTime,
      allDay: event.allDay,
      location: event.location,
      timeZone: event.timeZone,
      description: event.description,
      duration: event.duration,
      reminderMinutes: event.reminderMinutes,
      createdAt: Date.now(),
    });

    await this.sendNotification("Calendar Event Created", event);
    this.logger.info(`Created: "${event.title}" on ${event.startDate}`);
  }

  private async handleCancel(
    event: ExtractedEvent,
    existingById: Map<string, CreatedCalendarEventData>,
  ): Promise<void> {
    const record = resolveEventReference(event, existingById);

    if (!record) {
      this.logger.warn(
        `Cancel requested for unknown event: "${event.title}" (skipping)`,
      );
      return;
    }

    if (!this.calendarUrl) {
      this.logger.error("Calendar URL not discovered, cannot cancel event");
      return;
    }

    const result = await deleteCalendarEvent(
      this.calendarUrl,
      record.calendarEventId,
      this.logger,
    );

    if (result.status === "error") {
      this.logger.error(
        `Failed to delete calendar event "${event.title}": ${result.message}`,
      );
      return;
    }

    markEventCancelled(record.eventHash);

    await this.sendNotification("Calendar Event Cancelled", event);
    this.logger.info(`Cancelled: "${event.title}" on ${record.startDate}`);
  }

  private async handleUpdate(
    event: ExtractedEvent,
    existingById: Map<string, CreatedCalendarEventData>,
    emailId: string,
  ): Promise<void> {
    const record = resolveEventReference(event, existingById);

    if (!record) {
      this.logger.warn(
        `Update requested for unknown event: "${event.title}", treating as create`,
      );
      return this.handleCreate(event, emailId);
    }

    // The model can't see description/duration/reminderMinutes (they aren't in the
    // existing-event context), so a full-PUT update would silently drop them. Backfill
    // from the stored record for any field the model didn't restate, then compare the
    // merged result so an unseen field never reads as a spurious change.
    const merged: ExtractedEvent = {
      ...event,
      description: event.description ?? record.description,
      duration: event.duration ?? record.duration,
      reminderMinutes: event.reminderMinutes ?? record.reminderMinutes,
    };

    // Skip if nothing meaningful changed
    if (!hasEventChanged(record, merged)) {
      this.logger.info(
        `No changes detected for "${event.title}" on ${event.startDate} (skipping update)`,
      );
      return;
    }

    if (!this.calendarUrl) {
      this.logger.error("Calendar URL not discovered, cannot update event");
      return;
    }

    const result = await updateCalendarEvent(
      this.calendarUrl,
      merged,
      record.calendarEventId,
      this.logger,
    );

    if (result.status === "error") {
      this.logger.error(
        `Failed to update calendar event "${event.title}": ${result.message}`,
      );
      return;
    }

    // Mark old record as cancelled, create new one with updated hash
    markEventCancelled(record.eventHash);
    const newHash = computeEventHash(merged.title, merged.startDate, merged.startTime);
    recordCreatedEvent({
      eventHash: newHash,
      emailId,
      calendarEventId: record.calendarEventId,
      title: merged.title,
      startDate: merged.startDate,
      startTime: merged.startTime,
      endDate: merged.endDate,
      endTime: merged.endTime,
      allDay: merged.allDay,
      location: merged.location,
      timeZone: merged.timeZone,
      description: merged.description,
      duration: merged.duration,
      reminderMinutes: merged.reminderMinutes,
      createdAt: Date.now(),
    });

    await this.sendNotification("Calendar Event Updated", merged);
    this.logger.info(`Updated: "${event.title}" on ${event.startDate}`);
  }

  private async sendNotification(title: string, event: ExtractedEvent): Promise<void> {
    const timePart = event.allDay
      ? "(all day)"
      : event.startTime
        ? `at ${event.startTime}`
        : "";
    try {
      await notify({
        title,
        message: `${event.title}\n${event.startDate}${timePart ? ` ${timePart}` : ""}${event.location ? `\n${event.location}` : ""}`,
        token: config.PUSHOVER_CALENDAR_TOKEN,
      });
    } catch (error) {
      this.logger.warn("Failed to send notification", (error as Error).message);
    }
  }
}
