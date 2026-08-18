import type { Logger } from "@micthiesen/mitools/logging";
import type { CalendarEventExtraction } from "../extraction/schema.js";
import { buildICalendar, generateUid } from "./ics.js";

/** Resolved CalDAV target: a calendar collection URL plus the auth to use. */
export interface CaldavSession {
  /** Absolute collection URL, ending with "/". */
  calendarUrl: string;
  authHeader: string;
}

type CreateResult =
  | { status: "success"; eventUid: string }
  | { status: "error"; code: number; message: string };

type DeleteResult =
  | { status: "success" }
  | { status: "not_found" }
  | { status: "error"; code: number; message: string };

/** Create a calendar event via CalDAV PUT with an iCalendar body. */
export async function createCalendarEvent(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  logger: Logger,
  eventUid = generateUid(),
): Promise<CreateResult> {
  const uid = eventUid;
  const icsBody = buildICalendar(event, uid);
  const eventUrl = `${session.calendarUrl}${uid}.ics`;

  logger.debug(`CalDAV PUT ${eventUrl}\n${icsBody}`);

  const response = await fetch(eventUrl, {
    method: "PUT",
    headers: {
      "Content-Type": "text/calendar; charset=utf-8",
      Authorization: session.authHeader,
      "If-None-Match": "*", // Only create, don't overwrite
    },
    body: icsBody,
  });

  if (response.status === 201 || response.status === 204) {
    logger.info(`Created calendar event: ${event.title} (${uid})`);
    return { status: "success", eventUid: uid };
  }

  const text = await response.text();
  logger.error(
    `CalDAV PUT failed: ${response.status} ${response.statusText}`,
    `URL: ${eventUrl}\nBody:\n${icsBody}\nResponse:\n${text}`,
  );
  return {
    status: "error",
    code: response.status,
    message: `CalDAV ${response.status}: ${response.statusText}`,
  };
}

/** Update an existing calendar event via CalDAV PUT (overwrites). */
export async function updateCalendarEvent(
  session: CaldavSession,
  event: CalendarEventExtraction["events"][number],
  existingUid: string,
  logger: Logger,
): Promise<CreateResult> {
  const icsBody = buildICalendar(event, existingUid);
  const eventUrl = `${session.calendarUrl}${existingUid}.ics`;

  logger.debug(`CalDAV PUT (update) ${eventUrl}\n${icsBody}`);

  const response = await fetch(eventUrl, {
    method: "PUT",
    headers: {
      "Content-Type": "text/calendar; charset=utf-8",
      Authorization: session.authHeader,
    },
    body: icsBody,
  });

  if (response.status === 201 || response.status === 204) {
    logger.info(`Updated calendar event: ${event.title} (${existingUid})`);
    return { status: "success", eventUid: existingUid };
  }

  const text = await response.text();
  logger.error(
    `CalDAV PUT (update) failed: ${response.status} ${response.statusText}`,
    `URL: ${eventUrl}\nBody:\n${icsBody}\nResponse:\n${text}`,
  );
  return {
    status: "error",
    code: response.status,
    message: `CalDAV ${response.status}: ${response.statusText}`,
  };
}

/** Delete a calendar event via CalDAV DELETE. */
export async function deleteCalendarEvent(
  session: CaldavSession,
  uid: string,
  logger: Logger,
): Promise<DeleteResult> {
  const eventUrl = `${session.calendarUrl}${uid}.ics`;

  logger.debug(`CalDAV DELETE ${eventUrl}`);

  const response = await fetch(eventUrl, {
    method: "DELETE",
    headers: { Authorization: session.authHeader },
  });

  if (response.status === 204 || response.status === 200) {
    logger.info(`Deleted calendar event: ${uid}`);
    return { status: "success" };
  }

  if (response.status === 404) {
    logger.info(`Calendar event already gone: ${uid}`);
    return { status: "not_found" };
  }

  const text = await response.text();
  logger.error(
    `CalDAV DELETE failed: ${response.status} ${response.statusText}`,
    `URL: ${eventUrl}\nResponse:\n${text}`,
  );
  return {
    status: "error",
    code: response.status,
    message: `CalDAV ${response.status}: ${response.statusText}`,
  };
}
