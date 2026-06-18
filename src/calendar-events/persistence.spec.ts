import { describe, expect, it, vi } from "vitest";
import {
  type CreatedCalendarEventData,
  computeEventHash,
  hasEventChanged,
  normalizeTitle,
  resolveEventReference,
} from "./persistence.js";

const rec = (
  overrides: Partial<CreatedCalendarEventData>,
): CreatedCalendarEventData => ({
  eventHash: "h",
  emailId: "e",
  calendarEventId: "cal",
  title: "🦷 Dentist Appointment",
  startDate: "2026-06-17",
  allDay: false,
  createdAt: 0,
  ...overrides,
});

describe("normalizeTitle", () => {
  it("collapses emoji, arrow style, casing, and whitespace drift to one form", () => {
    const variants = [
      "✈️ Flight YYZ → YVR",
      "Flight YYZ -> YVR",
      "flight   yyz  yvr",
      "✈️  FLIGHT YYZ → YVR!",
    ];
    const normalized = variants.map(normalizeTitle);
    expect(new Set(normalized).size).toBe(1);
    expect(normalized[0]).toBe("flight yyz yvr");
  });

  it("keeps genuinely different titles distinct", () => {
    expect(normalizeTitle("🦷 Dentist Appointment")).not.toBe(
      normalizeTitle("🛂 Passport Renewal"),
    );
  });

  it("does not collapse same-token titles that differ only in order", () => {
    expect(normalizeTitle("✈️ Flight YYZ → YVR")).not.toBe(
      normalizeTitle("✈️ Flight YVR → YYZ"),
    );
  });
});

describe("computeEventHash", () => {
  it("is stable across title drift for the same date/time", () => {
    expect(computeEventHash("✈️ Flight YYZ → YVR", "2026-07-02", "08:00")).toBe(
      computeEventHash("Flight YYZ -> YVR", "2026-07-02", "08:00"),
    );
  });

  it("differs when the date or time differs", () => {
    expect(computeEventHash("🦷 Dentist", "2026-06-17", "09:00")).not.toBe(
      computeEventHash("🦷 Dentist", "2026-06-17", "10:00"),
    );
  });
});

describe("hasEventChanged", () => {
  const base = rec({
    title: "🦷 Dentist Appointment",
    startDate: "2026-06-17",
    startTime: "14:30",
    location: "Clinic",
    description: "Bring insurance card",
    reminderMinutes: 60,
  });

  it("returns false when nothing meaningful changed", () => {
    expect(hasEventChanged(base, { ...base })).toBe(false);
  });

  it("treats matching undefined optional fields as unchanged", () => {
    const a = rec({ title: "X", startDate: "2026-06-17", allDay: true });
    expect(
      hasEventChanged(a, { title: "X", startDate: "2026-06-17", allDay: true }),
    ).toBe(false);
  });

  it.each([
    ["title (semantic)", { title: "🦷 Dentist Checkup" }],
    ["startTime", { startTime: "15:00" }],
    ["location", { location: "New Clinic" }],
    ["description", { description: "Changed" }],
    ["duration", { duration: "PT2H" }],
    ["reminderMinutes", { reminderMinutes: 1440 }],
  ])("detects a change to %s", (_label, patch) => {
    expect(hasEventChanged(base, { ...base, ...patch })).toBe(true);
  });

  it("ignores cosmetic title drift (emoji/punctuation)", () => {
    expect(hasEventChanged(base, { ...base, title: "Dentist Appointment!" })).toBe(
      false,
    );
  });
});

describe("resolveEventReference", () => {
  it("prefers the eventId handle over a fallback that would also match", () => {
    const target = rec({ eventHash: "by-id", calendarEventId: "cal-1" });
    const other = rec({ eventHash: "by-fallback", calendarEventId: "cal-2" });
    const byId = new Map([["evt_2", target]]);
    const fallback = vi.fn().mockReturnValue(other);

    const result = resolveEventReference(
      {
        eventId: "evt_2",
        title: "completely different title",
        startDate: "2099-01-01",
      },
      byId,
      fallback,
    );

    expect(result).toBe(target);
    expect(fallback).not.toHaveBeenCalled();
  });

  it("tolerates a handle echoed back with surrounding brackets", () => {
    const target = rec({ eventHash: "by-id" });
    const result = resolveEventReference(
      { eventId: "[evt_2]", title: "x", startDate: "2026-06-17" },
      new Map([["evt_2", target]]),
      () => undefined,
    );

    expect(result).toBe(target);
  });

  it("falls back to title+startDate when eventId is missing", () => {
    const found = rec({ eventHash: "by-title" });
    const fallback = vi.fn().mockReturnValue(found);

    const result = resolveEventReference(
      { title: "🦷 Dentist Appointment", startDate: "2026-06-17" },
      new Map(),
      fallback,
    );

    expect(result).toBe(found);
    expect(fallback).toHaveBeenCalledWith("🦷 Dentist Appointment", "2026-06-17");
  });

  it("falls back when eventId is hallucinated (not in the map)", () => {
    const found = rec({ eventHash: "by-title" });
    const fallback = vi.fn().mockReturnValue(found);

    const result = resolveEventReference(
      { eventId: "evt_99", title: "🦷 Dentist Appointment", startDate: "2026-06-17" },
      new Map([["evt_1", rec({})]]),
      fallback,
    );

    expect(result).toBe(found);
    expect(fallback).toHaveBeenCalledOnce();
  });

  it("returns undefined when neither the handle nor the fallback matches", () => {
    const result = resolveEventReference(
      { eventId: "evt_99", title: "Unknown", startDate: "2026-06-17" },
      new Map(),
      () => undefined,
    );

    expect(result).toBeUndefined();
  });
});
