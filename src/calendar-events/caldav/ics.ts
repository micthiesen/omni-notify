import config from "../../utils/config.js";
import type { CalendarEventExtraction } from "../extraction/schema.js";

/** Build the iCalendar body for an extracted event. Pure; exported for testing. */
export function buildICalendar(
  event: CalendarEventExtraction["events"][number],
  uid: string,
  generatedAt: number,
): string {
  const tz = event.timeZone ?? config.TZ;
  const lines: string[] = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//omni-notify//EN",
    "CALSCALE:GREGORIAN",
    "BEGIN:VEVENT",
    `UID:${uid}`,
    `DTSTAMP:${formatUtc(generatedAt)}`,
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

  if (event.recurrence) {
    const freq = event.recurrence.frequency.toUpperCase();
    const until = event.allDay
      ? event.recurrence.until.replace(/-/g, "")
      : formatUtcUntil(event.recurrence.until, event.startTime ?? "00:00", tz);
    lines.push(`RRULE:FREQ=${freq};UNTIL=${until}`);
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

export function generateUid(now: number, randomValue: number): string {
  const timestamp = now.toString(36);
  const random = randomValue.toString(36).slice(2, 10);
  return `${timestamp}-${random}@omni-notify`;
}

function formatUtc(timestamp: number): string {
  const now = new Date(timestamp);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}T${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}Z`;
}

/**
 * RRULE UNTIL for a timed event must be a UTC date-time (RFC 5545) that covers
 * the last occurrence: the event's wall-clock start time on the until date,
 * converted from the event's timezone to UTC.
 */
function formatUtcUntil(
  untilDate: string,
  startTime: string,
  timeZone: string,
): string {
  const naive = new Date(`${untilDate}T${startTime}:00Z`);
  let instant = naive;
  try {
    const dtf = new Intl.DateTimeFormat("en-US", {
      timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hour12: false,
    });
    const parts: Record<string, string> = {};
    for (const part of dtf.formatToParts(naive)) parts[part.type] = part.value;
    const asUtc = Date.UTC(
      Number(parts.year),
      Number(parts.month) - 1,
      Number(parts.day),
      Number(parts.hour) % 24,
      Number(parts.minute),
      Number(parts.second),
    );
    // asUtc - naive is the zone's offset at that instant; subtract to get UTC.
    instant = new Date(naive.getTime() - (asUtc - naive.getTime()));
  } catch {
    // Unresolvable zone: fall back to treating the wall-clock time as UTC.
  }
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${instant.getUTCFullYear()}${pad(instant.getUTCMonth() + 1)}${pad(instant.getUTCDate())}T${pad(instant.getUTCHours())}${pad(instant.getUTCMinutes())}${pad(instant.getUTCSeconds())}Z`;
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
