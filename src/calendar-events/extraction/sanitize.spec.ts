import { describe, expect, it } from "vitest";
import {
  isDegenerateExtraction,
  isValidTimeZone,
  MAX_DESCRIPTION_CHARS,
  MAX_LOCATION_CHARS,
  MAX_TITLE_CHARS,
  sanitizeExtractedEvents,
  sanitizeTimeZone,
  truncated,
} from "./sanitize.js";
import type { ExtractedCalendarEvent } from "./schema.js";

const evt = (overrides: Partial<ExtractedCalendarEvent>): ExtractedCalendarEvent => ({
  action: "create",
  title: "🦷 Dentist Appointment",
  startDate: "2026-06-17",
  startTime: "14:30",
  allDay: false,
  ...overrides,
});

describe("isValidTimeZone", () => {
  it("accepts canonical IANA zones", () => {
    expect(isValidTimeZone("America/Vancouver")).toBe(true);
    expect(isValidTimeZone("Europe/London")).toBe(true);
  });

  it("accepts resolvable aliases outside supportedValuesOf", () => {
    expect(isValidTimeZone("Asia/Calcutta")).toBe(true);
  });

  it("rejects garbage, including model field soup", () => {
    expect(isValidTimeZone("America/Nowhere")).toBe(false);
    expect(
      isValidTimeZone(
        "The event takes place at the community centre startTime 09:00 endTime 16:00 ".repeat(
          50,
        ),
      ),
    ).toBe(false);
  });
});

describe("sanitizeTimeZone", () => {
  it("passes valid zones through and drops invalid ones", () => {
    expect(sanitizeTimeZone("America/Toronto")).toBe("America/Toronto");
    expect(sanitizeTimeZone("not a zone; startDate 2026-07-06")).toBeUndefined();
    expect(sanitizeTimeZone(undefined)).toBeUndefined();
  });
});

describe("sanitizeExtractedEvents", () => {
  it("drops an invalid timeZone and reports it", () => {
    const soup = "paragraph of field soup ".repeat(160); // ~3900 chars
    const result = sanitizeExtractedEvents([evt({ timeZone: soup })]);

    expect(result.events[0].timeZone).toBeUndefined();
    expect(result.timeZonesDropped).toBe(1);
    expect(result.issues.some((i) => i.includes("invalid timeZone"))).toBe(true);
  });

  it("keeps a valid timeZone untouched", () => {
    const result = sanitizeExtractedEvents([evt({ timeZone: "America/Vancouver" })]);

    expect(result.events[0].timeZone).toBe("America/Vancouver");
    expect(result.timeZonesDropped).toBe(0);
    expect(result.issues).toEqual([]);
  });

  it("truncates over-long title, location, and description", () => {
    const result = sanitizeExtractedEvents([
      evt({
        title: "T".repeat(MAX_TITLE_CHARS + 50),
        location: "L".repeat(MAX_LOCATION_CHARS + 50),
        description: "D".repeat(MAX_DESCRIPTION_CHARS + 50),
      }),
    ]);

    const [event] = result.events;
    expect(event.title).toHaveLength(MAX_TITLE_CHARS);
    expect(event.location).toHaveLength(MAX_LOCATION_CHARS);
    expect(event.description).toHaveLength(MAX_DESCRIPTION_CHARS);
    expect(result.issues).toHaveLength(3);
  });

  it("collapses byte-identical duplicates to one (degenerate repetition)", () => {
    const repeated = Array.from({ length: 100 }, () => evt({}));
    const result = sanitizeExtractedEvents(repeated);

    expect(result.events).toHaveLength(1);
    expect(result.duplicatesCollapsed).toBe(99);
    expect(result.issues.some((i) => i.includes("99 byte-identical"))).toBe(true);
  });

  it("keeps distinct events intact when collapsing duplicates", () => {
    const a = evt({ title: "A" });
    const b = evt({ title: "B" });
    const result = sanitizeExtractedEvents([a, b, evt({ title: "A" })]);

    expect(result.events.map((e) => e.title)).toEqual(["A", "B"]);
    expect(result.duplicatesCollapsed).toBe(1);
  });

  it("forces allDay and clears time fields for a timed event with no startTime", () => {
    const result = sanitizeExtractedEvents([
      evt({
        startTime: undefined,
        endTime: "16:00",
        duration: "PT2H",
        allDay: false,
      }),
    ]);

    const [event] = result.events;
    expect(event.allDay).toBe(true);
    expect(event.startTime).toBeUndefined();
    expect(event.endTime).toBeUndefined();
    expect(event.duration).toBeUndefined();
    expect(result.issues.some((i) => i.includes("forced allDay"))).toBe(true);
  });

  it("leaves a genuine all-day event alone", () => {
    const result = sanitizeExtractedEvents([
      evt({ startTime: undefined, allDay: true }),
    ]);

    expect(result.events[0].allDay).toBe(true);
    expect(result.issues).toEqual([]);
  });

  it("normalizes recurrence null to undefined without reporting an issue", () => {
    const result = sanitizeExtractedEvents([evt({ recurrence: null })]);

    expect(result.events[0].recurrence).toBeUndefined();
    expect(result.issues).toEqual([]);
  });

  it("keeps a valid recurrence and drops one with a malformed until date", () => {
    const valid = evt({
      recurrence: { frequency: "daily", until: "2026-07-13" },
    });
    const invalid = evt({
      title: "Other",
      recurrence: { frequency: "daily", until: "sometime in July" },
    });
    const result = sanitizeExtractedEvents([valid, invalid]);

    expect(result.events[0].recurrence).toEqual({
      frequency: "daily",
      until: "2026-07-13",
    });
    expect(result.events[1].recurrence).toBeUndefined();
    expect(result.issues.some((i) => i.includes("invalid until"))).toBe(true);
  });

  it("returns an empty result for empty input", () => {
    const result = sanitizeExtractedEvents([]);

    expect(result.events).toEqual([]);
    expect(result.issues).toEqual([]);
  });
});

describe("isDegenerateExtraction", () => {
  it("flags any dropped timeZone", () => {
    const result = sanitizeExtractedEvents([evt({ timeZone: "garbage soup" })]);
    expect(isDegenerateExtraction(result)).toBe(true);
  });

  it("flags collapsing more than half the returned objects", () => {
    const result = sanitizeExtractedEvents([evt({}), evt({}), evt({})]);
    expect(result.duplicatesCollapsed).toBe(2);
    expect(isDegenerateExtraction(result)).toBe(true);
  });

  it("does not flag a clean extraction or mild duplication", () => {
    const clean = sanitizeExtractedEvents([evt({}), evt({ title: "Other" })]);
    expect(isDegenerateExtraction(clean)).toBe(false);

    const mild = sanitizeExtractedEvents([
      evt({}),
      evt({}),
      evt({ title: "B" }),
      evt({ title: "C" }),
    ]);
    expect(mild.duplicatesCollapsed).toBe(1);
    expect(isDegenerateExtraction(mild)).toBe(false);
  });

  it("does not flag truncation-only fixes", () => {
    const result = sanitizeExtractedEvents([
      evt({ title: "T".repeat(MAX_TITLE_CHARS + 1) }),
    ]);
    expect(isDegenerateExtraction(result)).toBe(false);
  });
});

describe("truncated", () => {
  it("truncates only when over the cap", () => {
    expect(truncated("short", 10)).toBe("short");
    expect(truncated("0123456789abc", 10)).toBe("0123456789");
  });
});
