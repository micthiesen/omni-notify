import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { notify } from "@micthiesen/mitools/pushover";
import { deriveItemsOutcome, recordEmailActivity } from "../jmap/activity.js";
import { withEmailLogCapture } from "../jmap/activityLogs.js";
import type { JmapContext } from "../jmap/client.js";
import type { EmailHandler } from "../jmap/dispatcher.js";
import type { FetchedEmail } from "../jmap/emailFetcher.js";
import { enqueueEmailRetry } from "../jmap/retry.js";
import type { EmailTriageService } from "../jmap/triage.js";
import config from "../utils/config.js";
import { downloadSupportedAttachments } from "./extraction/attachments.js";
import {
  type ExistingEventContext,
  extractCalendarEvents,
} from "./extraction/extractEvents.js";
import {
  MAX_LOCATION_CHARS,
  MAX_TITLE_CHARS,
  sanitizeTimeZone,
  truncated,
} from "./extraction/sanitize.js";
import type { ExtractedCalendarEvent } from "./extraction/schema.js";
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
  reconcileEventHashes,
  recordCreatedEvent,
  resolveEventReference,
  resolveExplicitEventReference,
} from "./persistence.js";

type ExtractedEvent = ExtractedCalendarEvent;

/** Outcome of one extracted event's create/cancel/update handling. */
interface ItemResult {
  /** Short result line for the activity record. */
  line: string;
  ok: boolean;
  /** Set when a retryable CalDAV failure (network error / 5xx) occurred. */
  transient?: string;
}

/** Network-shaped failures and server errors are retryable; 4xx are not. */
function isTransientCalDavCode(code: number): boolean {
  return code >= 500;
}

export class CalendarEventPipeline implements EmailHandler {
  public readonly name = "CalendarEvents";
  private logger: Logger;
  private ctx: JmapContext;
  private triage: EmailTriageService;
  private calendarUrl?: string;

  constructor(ctx: JmapContext, logger: Logger, triage: EmailTriageService) {
    this.ctx = ctx;
    this.logger = logger;
    this.triage = triage;
    const rekeyed = reconcileEventHashes();
    if (rekeyed > 0) {
      this.logger.info(`Reconciled ${rekeyed} calendar event hash(es) to new scheme`);
    }
  }

  async handleEmails(emails: FetchedEmail[]): Promise<void> {
    // Filter candidates. Each email is guarded individually: a throw here
    // must not reject the whole batch, because the dispatcher advances the
    // JMAP cursor regardless and the other emails would be lost silently.
    const candidates: { email: FetchedEmail; admitReason: string }[] = [];
    for (const email of emails) {
      let result: Awaited<ReturnType<typeof filterCalendarCandidate>>;
      try {
        result = await filterCalendarCandidate(email, this.triage);
      } catch (error) {
        this.logger.error(
          `Filter failed for "${email.subject}"`,
          (error as Error).message,
        );
        recordEmailActivity({
          pipeline: this.name,
          email,
          outcome: "error",
          detail: `filter failed: ${(error as Error).message}`,
        });
        continue;
      }
      if (result.pass) {
        this.logger.info(
          `Candidate (${result.reason}): "${email.subject}" from ${email.from}`,
        );
        candidates.push({ email, admitReason: result.reason });
      } else {
        this.logger.info(
          `Skipped (${result.reason}): "${email.subject}" from ${email.from}`,
        );
        recordEmailActivity({
          pipeline: this.name,
          email,
          outcome: "filtered",
          detail: result.reason,
        });
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
        // The JMAP cursor still advances, so these candidates won't be retried.
        for (const { email, admitReason } of candidates) {
          recordEmailActivity({
            pipeline: this.name,
            email,
            outcome: "error",
            detail: `calendar discovery failed: ${(error as Error).message}`,
            admitReason,
          });
        }
        return;
      }
    }

    // Process each candidate, capturing its log lines for the activity UI
    for (const { email, admitReason } of candidates) {
      const runLog = config.LOGS_PATH
        ? new LogFile(
            `${config.LOGS_PATH}/calendar-events/${logTimestamp()}.md`,
            "overwrite",
          )
        : undefined;
      await withEmailLogCapture(`${this.name}#${email.id}`, this.name, async () => {
        try {
          await this.processEmail(email, admitReason, runLog);
        } catch (error) {
          this.logger.error(
            `Failed to process email "${email.subject}"`,
            (error as Error).message,
          );
          recordEmailActivity({
            pipeline: this.name,
            email,
            outcome: "error",
            detail: (error as Error).message,
            admitReason,
          });
        }
      });
    }
  }

  private async processEmail(
    email: FetchedEmail,
    admitReason: string,
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

    // Provide existing events as context for cancel/update matching, each tagged with a
    // stable per-prompt handle (evt_N) the model echoes back to identify its target —
    // decoupling matching from the regenerated title. Fields are re-sanitized here so
    // historical poisoned rows (garbage timeZone, runaway text) can't re-enter prompts.
    const existingById = new Map<string, CreatedCalendarEventData>();
    const existingEvents: ExistingEventContext[] = getRecentEvents().map((e, i) => {
      const id = `evt_${i + 1}`;
      existingById.set(id, e);
      return {
        id,
        title: truncated(e.title, MAX_TITLE_CHARS),
        startDate: e.startDate,
        startTime: e.startTime,
        endDate: e.endDate,
        endTime: e.endTime,
        allDay: e.allDay,
        location:
          e.location === undefined
            ? undefined
            : truncated(e.location, MAX_LOCATION_CHARS),
        timeZone: sanitizeTimeZone(e.timeZone),
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
      recordEmailActivity({
        pipeline: this.name,
        email,
        outcome: "error",
        detail: `extraction failed: ${(error as Error).message}`,
        admitReason,
      });
      return;
    }

    if (events.length === 0) {
      this.logger.info(`No calendar events found in "${email.subject}"`);
      recordEmailActivity({
        pipeline: this.name,
        email,
        outcome: "no_matches",
        detail: "no calendar events found",
        admitReason,
      });
      return;
    }

    this.logger.info(`Found ${events.length} event(s) in "${email.subject}"`);

    const items: string[] = [];
    const itemsOk: boolean[] = [];
    const transientFailures: string[] = [];
    for (const event of events) {
      let result: ItemResult;
      try {
        switch (event.action) {
          case "create":
            result = await this.handleCreate(event, email.id);
            break;
          case "cancel":
            result = await this.handleCancel(event, existingById);
            break;
          case "update":
            result = await this.handleUpdate(event, existingById, email.id);
            break;
        }
      } catch (error) {
        // CalDAV calls throw on transport failures (fetch network errors), which
        // are retryable; HTTP-level failures come back as result objects instead.
        const message = (error as Error).message;
        this.logger.error(
          `Failed to process event "${event.title}" (${event.action})`,
          message,
        );
        result = {
          line: `"${event.title}" (${event.action}): failed (${message})`,
          ok: false,
          transient: message,
        };
      }
      items.push(result.line);
      itemsOk.push(result.ok);
      if (result.transient !== undefined) transientFailures.push(result.transient);
    }

    if (transientFailures.length > 0) {
      const reason = transientFailures.join("; ");
      this.logger.warn(
        `Transient CalDAV failure(s) for "${email.subject}"; queued for retry: ${reason}`,
      );
      enqueueEmailRetry({ pipeline: this.name, emailId: email.id, reason });
    }

    recordEmailActivity({
      pipeline: this.name,
      email,
      outcome: deriveItemsOutcome(itemsOk),
      items,
      admitReason,
    });
  }

  private async handleCreate(
    event: ExtractedEvent,
    emailId: string,
  ): Promise<ItemResult> {
    const eventHash = computeEventHash(event.title, event.startDate, event.startTime);
    const label = `"${event.title}" on ${event.startDate}`;

    if (hasCreatedEvent(eventHash)) {
      this.logger.info(
        `Duplicate event: "${event.title}" on ${event.startDate} (skipping)`,
      );
      return { line: `${label}: duplicate, skipped`, ok: true };
    }

    if (!this.calendarUrl) {
      this.logger.error("Calendar URL not discovered, cannot create event");
      return { line: `${label}: failed (calendar URL not discovered)`, ok: false };
    }

    const result = await createCalendarEvent(this.calendarUrl, event, this.logger);

    if (result.status === "error") {
      this.logger.error(
        `Failed to create calendar event "${event.title}": ${result.message}`,
      );
      return {
        line: `${label}: create failed (${result.message})`,
        ok: false,
        transient: isTransientCalDavCode(result.code) ? result.message : undefined,
      };
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
      recurrence: event.recurrence ?? undefined,
      createdAt: Date.now(),
    });

    await this.sendNotification("Calendar Event Created", event);
    this.logger.info(`Created: "${event.title}" on ${event.startDate}`);
    return { line: `${label}: created`, ok: true };
  }

  private async handleCancel(
    event: ExtractedEvent,
    existingById: Map<string, CreatedCalendarEventData>,
  ): Promise<ItemResult> {
    // Cancels are destructive, so they require the explicit evt_N handle — a
    // title-only match (e.g. a payment receipt echoing an upcoming appointment's
    // title) must never delete an event.
    const record = resolveExplicitEventReference(event, existingById);
    const label = `"${event.title}"`;

    if (!record) {
      this.logger.warn(
        `Cancel without explicit event reference: "${event.title}" (skipping)`,
      );
      return {
        line: `${label}: cancel without explicit reference, skipped`,
        ok: false,
      };
    }

    if (!this.calendarUrl) {
      this.logger.error("Calendar URL not discovered, cannot cancel event");
      return {
        line: `${label}: cancel failed (calendar URL not discovered)`,
        ok: false,
      };
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
      return {
        line: `${label}: cancel failed (${result.message})`,
        ok: false,
        transient: isTransientCalDavCode(result.code) ? result.message : undefined,
      };
    }

    markEventCancelled(record.eventHash);

    await this.sendNotification("Calendar Event Cancelled", event);
    this.logger.info(`Cancelled: "${event.title}" on ${record.startDate}`);
    return { line: `${label} on ${record.startDate}: cancelled`, ok: true };
  }

  private async handleUpdate(
    event: ExtractedEvent,
    existingById: Map<string, CreatedCalendarEventData>,
    emailId: string,
  ): Promise<ItemResult> {
    const record = resolveEventReference(event, existingById);
    const label = `"${event.title}" on ${event.startDate}`;

    if (!record) {
      this.logger.warn(
        `Update requested for unknown event: "${event.title}", treating as create`,
      );
      return this.handleCreate(event, emailId);
    }

    // The model can't see description/duration/reminderMinutes/recurrence (they aren't
    // in the existing-event context), so a full-PUT update would silently drop them.
    // Backfill from the stored record for any field the model didn't restate, then
    // compare the merged result so an unseen field never reads as a spurious change.
    const merged: ExtractedEvent = {
      ...event,
      description: event.description ?? record.description,
      duration: event.duration ?? record.duration,
      reminderMinutes: event.reminderMinutes ?? record.reminderMinutes,
      recurrence: event.recurrence ?? record.recurrence,
    };

    // Skip if nothing meaningful changed
    if (!hasEventChanged(record, merged)) {
      this.logger.info(
        `No changes detected for "${event.title}" on ${event.startDate} (skipping update)`,
      );
      return { line: `${label}: no changes, skipped`, ok: true };
    }

    if (!this.calendarUrl) {
      this.logger.error("Calendar URL not discovered, cannot update event");
      return {
        line: `${label}: update failed (calendar URL not discovered)`,
        ok: false,
      };
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
      return {
        line: `${label}: update failed (${result.message})`,
        ok: false,
        transient: isTransientCalDavCode(result.code) ? result.message : undefined,
      };
    }

    // Re-key the local record to the updated identity. If that key is already taken by a
    // *different* tracked event, re-keying would clobber it — keep the existing key in
    // that rare case (the CalDAV event is already updated regardless).
    const newHash = computeEventHash(merged.title, merged.startDate, merged.startTime);
    const collides = newHash !== record.eventHash && hasCreatedEvent(newHash);
    if (collides) {
      this.logger.warn(
        `Update for "${merged.title}" collides with another tracked event's key; keeping existing key`,
      );
    }
    markEventCancelled(record.eventHash);
    recordCreatedEvent({
      eventHash: collides ? record.eventHash : newHash,
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
      recurrence: merged.recurrence ?? undefined,
      createdAt: Date.now(),
    });

    await this.sendNotification("Calendar Event Updated", merged);
    this.logger.info(`Updated: "${event.title}" on ${event.startDate}`);
    return { line: `${label}: updated`, ok: true };
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
