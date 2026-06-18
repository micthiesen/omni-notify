import { describe, expect, it, vi } from "vitest";
import { type CreatedCalendarEventData, resolveEventReference } from "./persistence.js";

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

describe("resolveEventReference", () => {
  it("resolves by eventId handle when present and known, ignoring the title", () => {
    const target = rec({ eventHash: "by-id", calendarEventId: "cal-1" });
    const byId = new Map([["evt_2", target]]);
    const fallback = vi.fn();

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
