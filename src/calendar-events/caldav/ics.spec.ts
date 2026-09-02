import { describe, expect, it } from "vitest";
import type { ExtractedCalendarEvent } from "../extraction/schema.js";
import { buildICalendar, generateUid } from "./ics.js";

const GENERATED_AT = Date.UTC(2026, 8, 1, 12, 34, 56);

const evt = (overrides: Partial<ExtractedCalendarEvent>): ExtractedCalendarEvent => ({
  action: "create",
  title: "🚧 Elevator Maintenance",
  startDate: "2026-07-06",
  startTime: "09:00",
  endTime: "16:00",
  timeZone: "America/Vancouver",
  allDay: false,
  ...overrides,
});

const icsLines = (event: ExtractedCalendarEvent): string[] =>
  buildICalendar(event, "uid-1@omni-notify", GENERATED_AT).split("\r\n");

describe("buildICalendar recurrence", () => {
  it("renders the supplied generation time and deterministic UID inputs", () => {
    expect(icsLines(evt({}))).toContain("DTSTAMP:20260901T123456Z");
    expect(generateUid(1234, 0.5)).toBe("ya-i@omni-notify");
  });
  it("emits RRULE with a UTC UNTIL covering the last occurrence for timed events", () => {
    const lines = icsLines(
      evt({ recurrence: { frequency: "daily", until: "2026-07-13" } }),
    );

    // 09:00 America/Vancouver on Jul 13 is PDT (UTC-7) → 16:00Z.
    expect(lines).toContain("RRULE:FREQ=DAILY;UNTIL=20260713T160000Z");
    expect(lines).toContain("DTSTART;TZID=America/Vancouver:20260706T090000");
    expect(lines).toContain("DTEND;TZID=America/Vancouver:20260706T160000");
  });

  it("converts UNTIL correctly for zones ahead of UTC", () => {
    const lines = icsLines(
      evt({
        startDate: "2026-12-01",
        timeZone: "Asia/Tokyo",
        recurrence: { frequency: "weekly", until: "2026-12-15" },
      }),
    );

    // 09:00 Asia/Tokyo (UTC+9, no DST) → 00:00Z the same day.
    expect(lines).toContain("RRULE:FREQ=WEEKLY;UNTIL=20261215T000000Z");
  });

  it("emits a date-format UNTIL for all-day recurring events", () => {
    const lines = icsLines(
      evt({
        startTime: undefined,
        endTime: undefined,
        allDay: true,
        recurrence: { frequency: "monthly", until: "2026-10-01" },
      }),
    );

    expect(lines).toContain("RRULE:FREQ=MONTHLY;UNTIL=20261001");
    expect(lines).toContain("DTSTART;VALUE=DATE:20260706");
  });

  it("emits no RRULE when recurrence is absent or null", () => {
    for (const event of [evt({}), evt({ recurrence: null })]) {
      const rrule = icsLines(event).find((l) => l.startsWith("RRULE"));
      expect(rrule).toBeUndefined();
    }
  });
});
