import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { notify } from "@micthiesen/mitools/pushover";
import { Effect } from "effect";
import {
  type AdmitTier,
  deriveItemsOutcome,
  recordEmailActivity,
  sumCostCents,
} from "../email/activity.js";
import { withEmailLogCaptureEffect } from "../email/activityLogs.js";
import { EmailRetryPersistence } from "../email/retry.js";
import type { EmailTriageService } from "../email/triage.js";
import type { EmailHandler, EmailTransport, FetchedEmail } from "../email/types.js";
import config from "../utils/config.js";
import {
  type CaldavSession,
  createCalendarEventEffect,
  deleteCalendarEventEffect,
  discoverCaldavSessionEffect,
  updateCalendarEventEffect,
} from "./caldav/index.js";
import { downloadSupportedAttachmentsEffect } from "./extraction/attachments.js";
import {
  type ExistingEventContext,
  type ExtractCalendarEventsResult,
  extractCalendarEventsEffect,
} from "./extraction/extractEvents.js";
import {
  MAX_LOCATION_CHARS,
  MAX_TITLE_CHARS,
  sanitizeTimeZone,
  truncated,
} from "./extraction/sanitize.js";
import type { ExtractedCalendarEvent } from "./extraction/schema.js";
import { filterCalendarCandidateEffect } from "./filter/keywords.js";
import { CalendarPersistenceError } from "./effect.js";
import {
  type CreatedCalendarEventData,
  computeEventHash,
  computeCalendarEventUid,
  getRecentEventsEffect,
  hasCreatedEventEffect,
  hasEventChanged,
  markEventCancelledEffect,
  reconcileEventHashesEffect,
  recordCreatedEventEffect,
  replaceCreatedEventEffect,
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
  private transport: EmailTransport;
  private triage: EmailTriageService;
  private caldav?: CaldavSession;
  private persistenceInitialized = false;

  constructor(transport: EmailTransport, logger: Logger, triage: EmailTriageService) {
    this.transport = transport;
    this.logger = logger;
    this.triage = triage;
  }

  public handleEmailsEffect(
    emails: FetchedEmail[],
  ): Effect.Effect<void, CalendarPersistenceError> {
    return Effect.gen({ self: this }, function* () {
      if (!this.persistenceInitialized) {
        const rekeyed = yield* reconcileEventHashesEffect();
        this.persistenceInitialized = true;
        if (rekeyed > 0) {
          this.logger.info(
            `Reconciled ${rekeyed} calendar event hash(es) to new scheme`,
          );
        }
      }
      const candidates = yield* Effect.forEach(emails, (email) =>
        filterCalendarCandidateEffect(email, this.triage).pipe(
          Effect.map((result) => {
            if (result.pass) {
              this.logger.info(
                `Candidate (${result.reason}): "${email.subject}" from ${email.from}`,
              );
              return { email, admitReason: result.reason, admitTier: result.admitTier };
            } else {
              this.logger.info(
                `Skipped (${result.reason}): "${email.subject}" from ${email.from}`,
              );
              recordEmailActivity({
                pipeline: this.name,
                email,
                outcome: "filtered",
                detail: result.reason,
                // A triage-rejected email still incurred a paid LLM call; attribute
                // it (null when a cheaper tier rejected before triage ran).
                costCents: this.triage.getTriageCostCents(email.id),
              });
              return undefined;
            }
          }),
        ),
      ).pipe(Effect.map((items) => items.filter((item) => item !== undefined)));

      // Discover calendar URL once (lazy init + cache)
      if (candidates.length > 0 && !this.caldav) {
        const discovery = yield* Effect.result(
          discoverCaldavSessionEffect(this.logger),
        );
        if (discovery._tag === "Success") {
          this.caldav = discovery.success;
        } else {
          const error = discovery.failure;
          this.logger.error(
            "Failed to discover calendar URL, skipping batch",
            (error as Error).message,
          );
          // The email cursor still advances, so these candidates won't be retried.
          for (const { email, admitReason, admitTier } of candidates) {
            yield* EmailRetryPersistence.enqueue({
              pipeline: this.name,
              emailId: email.id,
              reason: `calendar discovery failed: ${(error as Error).message}`,
            }).pipe(
              Effect.mapError(
                (cause) =>
                  new CalendarPersistenceError({
                    operation: "enqueue calendar email retry",
                    cause,
                  }),
              ),
            );
            recordEmailActivity({
              pipeline: this.name,
              email,
              outcome: "error",
              detail: `calendar discovery failed: ${(error as Error).message}`,
              admitReason,
              admitTier,
              costCents: sumCostCents([this.triageCostCentsFor(email.id, admitTier)]),
            });
          }
          return;
        }
      }

      // Process each candidate, capturing its log lines for the activity UI
      yield* Effect.forEach(
        candidates,
        ({ email, admitReason, admitTier }) => {
          const runLog = config.LOGS_PATH
            ? new LogFile(
                `${config.LOGS_PATH}/calendar-events/${logTimestamp()}.md`,
                "overwrite",
              )
            : undefined;
          // Triage cost only counts toward this row when triage is what admitted
          // it; the shared EmailTriageService memoizes per email, so the same
          // triage cost may also appear on ParcelTracker's row for this email —
          // acceptable for per-email transparency (see EmailTriageService docs).
          const triageCostCents = this.triageCostCentsFor(email.id, admitTier);
          const program = this.processEmail(
            email,
            admitReason,
            admitTier,
            triageCostCents,
            runLog,
          );
          return withEmailLogCaptureEffect(
            `${this.name}#${email.id}`,
            this.name,
            () => program,
          );
        },
        { discard: true },
      );
    });
  }

  /** Triage cost is only attributable when triage is what admitted the candidate. */
  private triageCostCentsFor(
    emailId: string,
    admitTier: AdmitTier,
  ): number | null | undefined {
    return admitTier === "triage" ? this.triage.getTriageCostCents(emailId) : undefined;
  }

  private processEmail(
    email: FetchedEmail,
    admitReason: string,
    admitTier: AdmitTier,
    triageCostCents: number | null | undefined,
    runLog?: LogFile,
  ): Effect.Effect<void, CalendarPersistenceError> {
    return Effect.gen({ self: this }, function* () {
      this.logger.info(
        `Extracting events from: "${email.subject}" (from: ${email.from})`,
      );

      // Download supported attachments (PDFs, images)
      const downloaded = yield* downloadSupportedAttachmentsEffect(
        this.transport,
        email.attachments,
        this.logger,
      );

      // Provide existing events as context for cancel/update matching, each tagged with a
      // stable per-prompt handle (evt_N) the model echoes back to identify its target —
      // decoupling matching from the regenerated title. Fields are re-sanitized here so
      // historical poisoned rows (garbage timeZone, runaway text) can't re-enter prompts.
      const existingById = new Map<string, CreatedCalendarEventData>();
      const existingEvents: ExistingEventContext[] =
        (yield* getRecentEventsEffect()).map((e, i) => {
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

      let extraction: ExtractCalendarEventsResult;
      try {
        const extracted = yield* Effect.result(
          extractCalendarEventsEffect({
            email: {
              subject: email.subject,
              from: email.from,
              textBody: email.textBody,
            },
            logger: this.logger,
            logFile: runLog,
            attachments: downloaded.length > 0 ? downloaded : undefined,
            localTimeZone: config.TZ,
            existingEvents,
          }),
        );
        if (extracted._tag === "Failure") throw extracted.failure;
        extraction = extracted.success;
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
          admitTier,
          // Extraction threw, so its cost isn't known; only triage may count.
          costCents: sumCostCents([triageCostCents]),
        });
        if (
          typeof error === "object" &&
          error !== null &&
          "_tag" in error &&
          error._tag === "CalendarExtractionError" &&
          "transient" in error &&
          error.transient === true
        ) {
          yield* EmailRetryPersistence.enqueue({
            pipeline: this.name,
            emailId: email.id,
            reason: "message" in error ? String(error.message) : String(error),
          }).pipe(
            Effect.mapError(
              (cause) =>
                new CalendarPersistenceError({
                  operation: "enqueue calendar email retry",
                  cause,
                }),
            ),
          );
        }
        return;
      }

      const { events, costCents: extractionCostCents } = extraction;
      const costCents = sumCostCents([triageCostCents, extractionCostCents]);

      if (events.length === 0) {
        this.logger.info(`No calendar events found in "${email.subject}"`);
        recordEmailActivity({
          pipeline: this.name,
          email,
          outcome: "no_matches",
          detail: "no calendar events found",
          admitReason,
          admitTier,
          costCents,
        });
        return;
      }

      this.logger.info(`Found ${events.length} event(s) in "${email.subject}"`);

      const items: string[] = [];
      const itemsOk: boolean[] = [];
      const transientFailures: string[] = [];
      for (const event of events) {
        const operation = (() => {
          switch (event.action) {
            case "create":
              return this.handleCreate(event, email.id);
            case "cancel":
              return this.handleCancel(event, existingById);
            case "update":
              return this.handleUpdate(event, existingById, email.id);
          }
        })();
        const outcome = yield* Effect.result(operation);
        let result: ItemResult;
        if (outcome._tag === "Failure") {
          if (outcome.failure instanceof CalendarPersistenceError) {
            return yield* outcome.failure;
          }
          // CalDAV calls throw on transport failures (fetch network errors), which
          // are retryable; HTTP-level failures come back as result objects instead.
          const message = outcome.failure.message;
          this.logger.error(
            `Failed to process event "${event.title}" (${event.action})`,
            message,
          );
          result = {
            line: `"${event.title}" (${event.action}): failed (${message})`,
            ok: false,
            transient: message,
          };
        } else result = outcome.success;
        items.push(result.line);
        itemsOk.push(result.ok);
        if (result.transient !== undefined) transientFailures.push(result.transient);
      }

      if (transientFailures.length > 0) {
        const reason = transientFailures.join("; ");
        this.logger.warn(
          `Transient CalDAV failure(s) for "${email.subject}"; queued for retry: ${reason}`,
        );
        yield* EmailRetryPersistence.enqueue({
          pipeline: this.name,
          emailId: email.id,
          reason,
        }).pipe(
          Effect.mapError(
            (cause) =>
              new CalendarPersistenceError({
                operation: "enqueue calendar email retry",
                cause,
              }),
          ),
        );
      }

      recordEmailActivity({
        pipeline: this.name,
        email,
        outcome: deriveItemsOutcome(itemsOk),
        items,
        admitReason,
        admitTier,
        costCents,
      });
    });
  }

  private handleCreate(
    event: ExtractedEvent,
    emailId: string,
  ): Effect.Effect<
    ItemResult,
    import("./effect.js").CaldavError | CalendarPersistenceError
  > {
    return Effect.gen({ self: this }, function* () {
      const eventHash = computeEventHash(event.title, event.startDate, event.startTime);
      const label = `"${event.title}" on ${event.startDate}`;

      if (yield* hasCreatedEventEffect(eventHash)) {
        this.logger.info(
          `Duplicate event: "${event.title}" on ${event.startDate} (skipping)`,
        );
        return { line: `${label}: duplicate, skipped`, ok: true };
      }

      if (!this.caldav) {
        this.logger.error("Calendar URL not discovered, cannot create event");
        return { line: `${label}: failed (calendar URL not discovered)`, ok: false };
      }

      const eventUid = computeCalendarEventUid(eventHash);
      const result = yield* createCalendarEventEffect(
        this.caldav,
        event,
        this.logger,
        eventUid,
      );

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

      const createdAt = yield* Effect.clockWith((clock) => clock.currentTimeMillis);
      yield* recordCreatedEventEffect({
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
        createdAt,
      });

      yield* this.sendNotification("Calendar Event Created", event);
      this.logger.info(`Created: "${event.title}" on ${event.startDate}`);
      return { line: `${label}: created`, ok: true };
    });
  }

  private handleCancel(
    event: ExtractedEvent,
    existingById: Map<string, CreatedCalendarEventData>,
  ): Effect.Effect<
    ItemResult,
    import("./effect.js").CaldavError | CalendarPersistenceError
  > {
    return Effect.gen({ self: this }, function* () {
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

      if (!this.caldav) {
        this.logger.error("Calendar URL not discovered, cannot cancel event");
        return {
          line: `${label}: cancel failed (calendar URL not discovered)`,
          ok: false,
        };
      }

      const result = yield* deleteCalendarEventEffect(
        this.caldav,
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

      yield* markEventCancelledEffect(record.eventHash);

      yield* this.sendNotification("Calendar Event Cancelled", event);
      this.logger.info(`Cancelled: "${event.title}" on ${record.startDate}`);
      return { line: `${label} on ${record.startDate}: cancelled`, ok: true };
    });
  }

  private handleUpdate(
    event: ExtractedEvent,
    existingById: Map<string, CreatedCalendarEventData>,
    emailId: string,
  ): Effect.Effect<
    ItemResult,
    import("./effect.js").CaldavError | CalendarPersistenceError
  > {
    return Effect.gen({ self: this }, function* () {
      const record = resolveEventReference(event, existingById);
      const label = `"${event.title}" on ${event.startDate}`;

      if (!record) {
        this.logger.warn(
          `Update requested for unknown event: "${event.title}", treating as create`,
        );
        return yield* this.handleCreate(event, emailId);
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

      if (!this.caldav) {
        this.logger.error("Calendar URL not discovered, cannot update event");
        return {
          line: `${label}: update failed (calendar URL not discovered)`,
          ok: false,
        };
      }

      const result = yield* updateCalendarEventEffect(
        this.caldav,
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
      const newHash = computeEventHash(
        merged.title,
        merged.startDate,
        merged.startTime,
      );
      const collides =
        newHash !== record.eventHash && (yield* hasCreatedEventEffect(newHash));
      if (collides) {
        this.logger.warn(
          `Update for "${merged.title}" collides with another tracked event's key; keeping existing key`,
        );
      }
      yield* replaceCreatedEventEffect(
        {
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
          createdAt: yield* Effect.clockWith((clock) => clock.currentTimeMillis),
        },
        record.eventHash,
      );

      yield* this.sendNotification("Calendar Event Updated", merged);
      this.logger.info(`Updated: "${event.title}" on ${event.startDate}`);
      return { line: `${label}: updated`, ok: true };
    });
  }

  private sendNotification(
    title: string,
    event: ExtractedEvent,
  ): Effect.Effect<void, never> {
    const timePart = event.allDay
      ? "(all day)"
      : event.startTime
        ? `at ${event.startTime}`
        : "";
    return Effect.tryPromise({
      try: () =>
        notify({
          title,
          message: `${event.title}\n${event.startDate}${timePart ? ` ${timePart}` : ""}${event.location ? `\n${event.location}` : ""}`,
          token: config.PUSHOVER_CALENDAR_TOKEN,
        }),
      catch: (cause) => cause,
    }).pipe(
      Effect.catch((error) =>
        Effect.sync(() =>
          this.logger.warn(
            "Failed to send notification",
            error instanceof Error ? error.message : String(error),
          ),
        ),
      ),
    );
  }
}
