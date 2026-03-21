import type { Logger } from "@micthiesen/mitools/logging";
import config from "../../utils/config.js";
import type { CalendarEventExtraction } from "../extraction/schema.js";

const CALDAV_HOST = "https://caldav.fastmail.com";
const CALDAV_BASE = `${CALDAV_HOST}/dav/calendars`;

type CreateResult =
  | { status: "success"; eventUid: string }
  | { status: "error"; message: string };

type DeleteResult =
  | { status: "success" }
  | { status: "not_found" }
  | { status: "error"; message: string };

/**
 * Discover the default calendar URL via CalDAV PROPFIND.
 * Returns the URL path to use for creating events.
 */
export async function discoverCalendarUrl(logger: Logger): Promise<string> {
  if (config.FASTMAIL_CALENDAR_ID) {
    const url = `${CALDAV_BASE}/user/${config.FASTMAIL_USERNAME}/${config.FASTMAIL_CALENDAR_ID}/`;
    logger.info(`Using configured calendar: ${url}`);
    return url;
  }

  // PROPFIND on the user's calendar home to find the default calendar
  const homeUrl = `${CALDAV_BASE}/user/${config.FASTMAIL_USERNAME}/`;
  const response = await fetch(homeUrl, {
    method: "PROPFIND",
    headers: {
      "Content-Type": "application/xml",
      Authorization: caldavAuth(),
      Depth: "1",
    },
    body: `<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
  </d:prop>
</d:propfind>`,
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(
      `CalDAV PROPFIND failed: ${response.status} ${response.statusText}\n${text}`,
    );
  }

  const xml = await response.text();

  // Parse the multistatus response to find calendar collections
  // Look for responses that have <resourcetype><calendar/></resourcetype>
  const calendarUrls = extractCalendarUrls(xml);

  if (calendarUrls.length === 0) {
    throw new Error("No calendars found via CalDAV PROPFIND");
  }

  // Prefer "Default" or "Personal" calendar, otherwise use first
  const preferred = calendarUrls.find(
    (c) => c.name.toLowerCase() === "default" || c.name.toLowerCase() === "personal",
  );
  const selected = preferred ?? calendarUrls[0];
  const calendarUrl = `${CALDAV_HOST}${selected.href}`;
  logger.info(`Using calendar: ${selected.name} (${calendarUrl})`);
  return calendarUrl;
}

/** Create a calendar event via CalDAV PUT with an iCalendar body. */
export async function createCalendarEvent(
  calendarUrl: string,
  event: CalendarEventExtraction["events"][number],
  logger: Logger,
): Promise<CreateResult> {
  const uid = generateUid();
  const icsBody = buildICalendar(event, uid);
  const eventUrl = `${calendarUrl}${uid}.ics`;

  logger.debug(`CalDAV PUT ${eventUrl}\n${icsBody}`);

  const response = await fetch(eventUrl, {
    method: "PUT",
    headers: {
      "Content-Type": "text/calendar; charset=utf-8",
      Authorization: caldavAuth(),
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
    message: `CalDAV ${response.status}: ${response.statusText}`,
  };
}

/** Update an existing calendar event via CalDAV PUT (overwrites). */
export async function updateCalendarEvent(
  calendarUrl: string,
  event: CalendarEventExtraction["events"][number],
  existingUid: string,
  logger: Logger,
): Promise<CreateResult> {
  const icsBody = buildICalendar(event, existingUid);
  const eventUrl = `${calendarUrl}${existingUid}.ics`;

  logger.debug(`CalDAV PUT (update) ${eventUrl}\n${icsBody}`);

  const response = await fetch(eventUrl, {
    method: "PUT",
    headers: {
      "Content-Type": "text/calendar; charset=utf-8",
      Authorization: caldavAuth(),
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
    message: `CalDAV ${response.status}: ${response.statusText}`,
  };
}

/** Delete a calendar event via CalDAV DELETE. */
export async function deleteCalendarEvent(
  calendarUrl: string,
  uid: string,
  logger: Logger,
): Promise<DeleteResult> {
  const eventUrl = `${calendarUrl}${uid}.ics`;

  logger.debug(`CalDAV DELETE ${eventUrl}`);

  const response = await fetch(eventUrl, {
    method: "DELETE",
    headers: { Authorization: caldavAuth() },
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
    message: `CalDAV ${response.status}: ${response.statusText}`,
  };
}

function buildICalendar(
  event: CalendarEventExtraction["events"][number],
  uid: string,
): string {
  const tz = event.timeZone ?? config.TZ;
  const lines: string[] = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//omni-notify//EN",
    "CALSCALE:GREGORIAN",
    "BEGIN:VEVENT",
    `UID:${uid}`,
    `DTSTAMP:${formatUtcNow()}`,
    `SUMMARY:${escapeIcal(event.title)}`,
  ];

  // Start time
  if (event.allDay) {
    lines.push(`DTSTART;VALUE=DATE:${event.startDate.replace(/-/g, "")}`);
    // iCal all-day DTEND is exclusive, so single-day = start + 1 day
    const endDate = event.endDate ?? event.startDate;
    lines.push(`DTEND;VALUE=DATE:${nextDay(endDate).replace(/-/g, "")}`);
  } else {
    const startTime = event.startTime ?? "00:00";
    const dtstart = `${event.startDate.replace(/-/g, "")}T${startTime.replace(":", "")}00`;
    lines.push(`DTSTART;TZID=${tz}:${dtstart}`);

    // End time or duration
    const endDateStr = event.endDate ?? event.startDate;
    if (event.endTime) {
      const dtend = `${endDateStr.replace(/-/g, "")}T${event.endTime.replace(":", "")}00`;
      lines.push(`DTEND;TZID=${tz}:${dtend}`);
    } else if (event.duration) {
      lines.push(`DURATION:${event.duration}`);
    } else {
      lines.push("DURATION:PT1H");
    }
  }

  if (event.location) {
    lines.push(`LOCATION:${escapeIcal(event.location)}`);
  }

  if (event.description) {
    lines.push(`DESCRIPTION:${escapeIcal(event.description)}`);
  }

  // Reminder alarm (LLM-chosen or default 30 min)
  if (!event.allDay) {
    const mins = event.reminderMinutes ?? 30;
    const trigger =
      mins >= 60
        ? `PT${Math.floor(mins / 60)}H${mins % 60 ? `${mins % 60}M` : ""}`
        : `PT${mins}M`;
    lines.push("BEGIN:VALARM", `TRIGGER:-${trigger}`, "ACTION:DISPLAY", "END:VALARM");
  }

  lines.push("END:VEVENT", "END:VCALENDAR");
  return lines.join("\r\n");
}

function caldavAuth(): string {
  const username = config.FASTMAIL_USERNAME ?? "";
  const password = config.FASTMAIL_APP_PASSWORD ?? "";
  return `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`;
}

function generateUid(): string {
  const timestamp = Date.now().toString(36);
  const random = Math.random().toString(36).slice(2, 10);
  return `${timestamp}-${random}@omni-notify`;
}

function formatUtcNow(): string {
  const now = new Date();
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}T${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}Z`;
}

/** Add one day to an ISO 8601 date string (e.g. "2026-03-20" → "2026-03-21"). */
function nextDay(dateStr: string): string {
  const d = new Date(`${dateStr}T00:00:00`);
  d.setDate(d.getDate() + 1);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Escape special characters for iCalendar text values. */
function escapeIcal(text: string): string {
  return text
    .replace(/\\/g, "\\\\")
    .replace(/;/g, "\\;")
    .replace(/,/g, "\\,")
    .replace(/\n/g, "\\n");
}

/** Extract calendar URLs from a PROPFIND multistatus XML response. */
function extractCalendarUrls(xml: string): { href: string; name: string }[] {
  const results: { href: string; name: string }[] = [];

  // Split on <response> or <d:response> blocks
  const responseBlocks = xml.split(/<(?:d:|D:)?response>/i).slice(1);

  for (const block of responseBlocks) {
    // Check if this is a calendar resource (has <calendar/> in resourcetype)
    const isCalendar =
      /<(?:c:|cal:)?calendar\s*\/>/i.test(block) ||
      /urn:ietf:params:xml:ns:caldav.*calendar/i.test(block);
    if (!isCalendar) continue;

    // Extract href
    const hrefMatch = block.match(/<(?:d:|D:)?href>([^<]+)<\/(?:d:|D:)?href>/i);
    if (!hrefMatch) continue;

    // Extract displayname (may be wrapped in CDATA)
    const nameMatch = block.match(
      /<(?:d:|D:)?displayname>(?:<!\[CDATA\[)?(.*?)(?:\]\]>)?<\/(?:d:|D:)?displayname>/i,
    );
    const name = nameMatch?.[1] ?? "Unnamed";

    results.push({ href: hrefMatch[1], name });
  }

  return results;
}
