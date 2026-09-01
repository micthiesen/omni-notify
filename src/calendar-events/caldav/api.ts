import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { runPromise } from "../../effect/interop.js";
import { CaldavError } from "../effect.js";
import type { CalendarEventExtraction } from "../extraction/schema.js";
import {
  CALDAV_ERROR_MAX_BYTES,
  readCaldavResponseText,
  requestCaldavEffect,
} from "./http.js";
import { buildICalendar, generateUid } from "./ics.js";

/** Resolved CalDAV target: a calendar collection URL plus the auth to use. */
export interface CaldavSession {
  /** Absolute collection URL, ending with "/". */
  calendarUrl: string;
  authHeader: string;
}

type CreateResult =
  | { status: "success"; eventUid: string }
  | { status: "already_exists"; eventUid: string }
  | { status: "error"; code: number; message: string };

type DeleteResult =
  | { status: "success" }
  | { status: "not_found" }
  | { status: "error"; code: number; message: string };

/** Create a calendar event via CalDAV PUT with an iCalendar body. */
export function createCalendarEventEffect(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  logger: Logger,
  eventUid = generateUid(),
): Effect.Effect<CreateResult, CaldavError> {
  const uid = eventUid;
  const icsBody = buildICalendar(event, uid);
  const eventUrl = `${session.calendarUrl}${uid}.ics`;

  logger.debug(`CalDAV PUT ${eventUrl}\n${icsBody}`);

  return requestCaldavEffect(
    eventUrl,
    {
      method: "PUT",
      headers: {
        "Content-Type": "text/calendar; charset=utf-8",
        Authorization: session.authHeader,
        "If-None-Match": "*", // Only create, don't overwrite
      },
      body: icsBody,
    },
    "create calendar event",
  ).pipe(
    Effect.flatMap((response) =>
      Effect.gen(function* () {
        if (response.status === 201 || response.status === 204) {
          logger.info(`Created calendar event: ${event.title} (${uid})`);
          return { status: "success", eventUid: uid };
        }

        // A deterministic UID lets callers reconcile a request whose response was
        // lost after the server committed the create.
        if (response.status === 412) {
          logger.info(`Calendar event already exists: ${uid}`);
          return { status: "already_exists", eventUid: uid };
        }

        const text = yield* readResponseText(response, "create calendar event");
        logger.error(
          `CalDAV PUT failed: ${response.status} ${response.statusText}`,
          `URL: ${eventUrl}\nBody:\n${icsBody}\nResponse:\n${text}`,
        );
        return {
          status: "error",
          code: response.status,
          message: `CalDAV ${response.status}: ${response.statusText}`,
        };
      }),
    ),
  );
}

export function createCalendarEvent(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  logger: Logger,
  eventUid = generateUid(),
): Promise<CreateResult> {
  return runPromise(createCalendarEventEffect(session, event, logger, eventUid));
}

/** Update an existing calendar event via CalDAV PUT (overwrites). */
export function updateCalendarEventEffect(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  existingUid: string,
  logger: Logger,
): Effect.Effect<CreateResult, CaldavError> {
  const icsBody = buildICalendar(event, existingUid);
  const eventUrl = `${session.calendarUrl}${existingUid}.ics`;

  logger.debug(`CalDAV PUT (update) ${eventUrl}\n${icsBody}`);

  return requestCaldavEffect(
    eventUrl,
    {
      method: "PUT",
      headers: {
        "Content-Type": "text/calendar; charset=utf-8",
        Authorization: session.authHeader,
      },
      body: icsBody,
    },
    "update calendar event",
  ).pipe(
    Effect.flatMap((response) =>
      Effect.gen(function* () {
        if (response.status === 201 || response.status === 204) {
          logger.info(`Updated calendar event: ${event.title} (${existingUid})`);
          return { status: "success", eventUid: existingUid };
        }

        const text = yield* readResponseText(response, "update calendar event");
        logger.error(
          `CalDAV PUT (update) failed: ${response.status} ${response.statusText}`,
          `URL: ${eventUrl}\nBody:\n${icsBody}\nResponse:\n${text}`,
        );
        return {
          status: "error",
          code: response.status,
          message: `CalDAV ${response.status}: ${response.statusText}`,
        };
      }),
    ),
  );
}

export function updateCalendarEvent(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  existingUid: string,
  logger: Logger,
): Promise<CreateResult> {
  return runPromise(updateCalendarEventEffect(session, event, existingUid, logger));
}

/** Delete a calendar event via CalDAV DELETE. */
export function deleteCalendarEventEffect(
  session: CaldavSession,
  uid: string,
  logger: Logger,
): Effect.Effect<DeleteResult, CaldavError> {
  const eventUrl = `${session.calendarUrl}${uid}.ics`;

  logger.debug(`CalDAV DELETE ${eventUrl}`);

  return requestCaldavEffect(
    eventUrl,
    {
      method: "DELETE",
      headers: { Authorization: session.authHeader },
    },
    "delete calendar event",
  ).pipe(
    Effect.flatMap((response) =>
      Effect.gen(function* () {
        if (response.status === 204 || response.status === 200) {
          logger.info(`Deleted calendar event: ${uid}`);
          return { status: "success" };
        }

        if (response.status === 404) {
          logger.info(`Calendar event already gone: ${uid}`);
          return { status: "not_found" };
        }

        const text = yield* readResponseText(response, "delete calendar event");
        logger.error(
          `CalDAV DELETE failed: ${response.status} ${response.statusText}`,
          `URL: ${eventUrl}\nResponse:\n${text}`,
        );
        return {
          status: "error",
          code: response.status,
          message: `CalDAV ${response.status}: ${response.statusText}`,
        };
      }),
    ),
  );
}

export function deleteCalendarEvent(
  session: CaldavSession,
  uid: string,
  logger: Logger,
): Promise<DeleteResult> {
  return runPromise(deleteCalendarEventEffect(session, uid, logger));
}

function readResponseText(
  response: Response,
  operation: string,
): Effect.Effect<string, CaldavError> {
  return readCaldavResponseText(response, operation, CALDAV_ERROR_MAX_BYTES);
}
