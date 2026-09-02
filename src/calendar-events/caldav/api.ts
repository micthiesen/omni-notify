import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Clock, Effect, Random } from "effect";
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

/** Create a calendar event via CalDAV PUT with an iCalendar body. */
export function createCalendarEventEffect(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  logger: NamedLogger,
  eventUid?: string,
) {
  return Effect.gen(function* () {
    const generatedAt = yield* Clock.currentTimeMillis;
    const uid = eventUid ?? generateUid(generatedAt, yield* Random.next);
    const icsBody = buildICalendar(event, uid, generatedAt);
    const eventUrl = `${session.calendarUrl}${uid}.ics`;
    yield* logger.debug(`CalDAV PUT ${eventUrl}\n${icsBody}`);
    const response = yield* requestCaldavEffect(
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
    );
    if (response.status === 201 || response.status === 204) {
      yield* logger.info(`Created calendar event: ${event.title} (${uid})`);
      return { status: "success", eventUid: uid } as const;
    }
    if (response.status === 412) {
      yield* logger.info(`Calendar event already exists: ${uid}`);
      return { status: "already_exists", eventUid: uid } as const;
    }
    const text = yield* readResponseText(response, "create calendar event");
    yield* logger.error(
      `CalDAV PUT failed: ${response.status} ${response.statusText}`,
      `URL: ${eventUrl}\nBody:\n${icsBody}\nResponse:\n${text}`,
    );
    return {
      status: "error",
      code: response.status,
      message: `CalDAV ${response.status}: ${response.statusText}`,
    } as const;
  });
}

/** Update an existing calendar event via CalDAV PUT (overwrites). */
export function updateCalendarEventEffect(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  existingUid: string,
  logger: NamedLogger,
) {
  return Effect.gen(function* () {
    const generatedAt = yield* Clock.currentTimeMillis;
    const icsBody = buildICalendar(event, existingUid, generatedAt);
    const eventUrl = `${session.calendarUrl}${existingUid}.ics`;
    yield* logger.debug(`CalDAV PUT (update) ${eventUrl}\n${icsBody}`);
    const response = yield* requestCaldavEffect(
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
    );
    if (response.status === 201 || response.status === 204) {
      yield* logger.info(`Updated calendar event: ${event.title} (${existingUid})`);
      return { status: "success", eventUid: existingUid } as const;
    }
    const text = yield* readResponseText(response, "update calendar event");
    yield* logger.error(
      `CalDAV PUT (update) failed: ${response.status} ${response.statusText}`,
      `URL: ${eventUrl}\nBody:\n${icsBody}\nResponse:\n${text}`,
    );
    return {
      status: "error",
      code: response.status,
      message: `CalDAV ${response.status}: ${response.statusText}`,
    } as const;
  });
}

/** Delete a calendar event via CalDAV DELETE. */
export function deleteCalendarEventEffect(
  session: CaldavSession,
  uid: string,
  logger: NamedLogger,
) {
  const eventUrl = `${session.calendarUrl}${uid}.ics`;

  return logger.debug(`CalDAV DELETE ${eventUrl}`).pipe(
    Effect.andThen(
      requestCaldavEffect(
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
              yield* logger.info(`Deleted calendar event: ${uid}`);
              return { status: "success" } as const;
            }

            if (response.status === 404) {
              yield* logger.info(`Calendar event already gone: ${uid}`);
              return { status: "not_found" } as const;
            }

            const text = yield* readResponseText(response, "delete calendar event");
            yield* logger.error(
              `CalDAV DELETE failed: ${response.status} ${response.statusText}`,
              `URL: ${eventUrl}\nResponse:\n${text}`,
            );
            return {
              status: "error",
              code: response.status,
              message: `CalDAV ${response.status}: ${response.statusText}`,
            } as const;
          }),
        ),
      ),
    ),
  );
}

function readResponseText(
  response: Response,
  operation: string,
): Effect.Effect<string, CaldavError> {
  return readCaldavResponseText(response, operation, CALDAV_ERROR_MAX_BYTES);
}
