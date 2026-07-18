import { describe, expect, it, vi } from "vitest";
import {
  type CreatedCalendarEventData,
  computeEventHash,
  findEvent,
  hasEventChanged,
  normalizeTitle,
  pickByStartDate,
  resolveEventReference,
  resolveExplicitEventReference,
  selectRecentEvents,
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

  it("detects added, removed, and modified recurrence", () => {
    const recurring = rec({
      ...base,
      recurrence: { frequency: "daily", until: "2026-07-13" },
    });

    expect(
      hasEventChanged(base, {
        ...base,
        recurrence: { frequency: "daily", until: "2026-07-13" },
      }),
    ).toBe(true);
    expect(hasEventChanged(recurring, { ...base })).toBe(true);
    expect(
      hasEventChanged(recurring, {
        ...base,
        recurrence: { frequency: "daily", until: "2026-07-20" },
      }),
    ).toBe(true);
  });

  it("treats identical recurrence (and null vs undefined) as unchanged", () => {
    const recurring = rec({
      ...base,
      recurrence: { frequency: "weekly", until: "2026-08-01" },
    });

    expect(
      hasEventChanged(recurring, {
        ...base,
        recurrence: { frequency: "weekly", until: "2026-08-01" },
      }),
    ).toBe(false);
    expect(hasEventChanged(base, { ...base, recurrence: null })).toBe(false);
  });
});

describe("selectRecentEvents", () => {
  const now = Date.parse("2026-04-15T12:00:00Z");
  const events = [
    rec({ eventHash: "old", startDate: "2026-04-01" }), // 14 days past
    rec({ eventHash: "recent-past", startDate: "2026-04-12" }), // 3 days past
    rec({ eventHash: "soon", startDate: "2026-05-01" }),
    rec({ eventHash: "september", startDate: "2026-09-10" }), // ~150 days ahead
    rec({ eventHash: "next-year", startDate: "2027-06-01" }), // beyond 365 days
    rec({ eventHash: "cancelled", startDate: "2026-05-01", status: "cancelled" }),
  ];

  it("includes far-future events within a 365-day horizon", () => {
    const selected = selectRecentEvents(events, 365, now);
    expect(selected.map((e) => e.eventHash)).toEqual([
      "recent-past",
      "soon",
      "september",
    ]);
  });

  it("hides far-future events under the old 90-day horizon (the dup-cluster bug)", () => {
    const selected = selectRecentEvents(events, 90, now);
    expect(selected.map((e) => e.eventHash)).toEqual(["recent-past", "soon"]);
  });
});

describe("findEvent", () => {
  it("matches title+date within the given candidate set", () => {
    const target = rec({
      eventHash: "t",
      title: "💇 Haircut",
      startDate: "2026-07-20",
    });
    const other = rec({ eventHash: "o", title: "💇 Haircut", startDate: "2026-08-20" });

    expect(findEvent("Haircut", "2026-07-20", [target, other])).toBe(target);
  });

  it("resolves a lone in-window candidate even when stale same-title events exist outside the set", () => {
    // Three stale past "haircut" events exist in the store but are NOT in the
    // windowed candidate set shown to the model — they must not break resolution.
    const inWindow = rec({
      eventHash: "current",
      title: "💇 Haircut",
      startDate: "2026-07-20",
    });

    expect(findEvent("Haircut", "2026-07-21", [inWindow])).toBe(inWindow);
  });

  it("fails closed when several in-window candidates share the title and none match the date", () => {
    const a = rec({ eventHash: "a", title: "💇 Haircut", startDate: "2026-07-20" });
    const b = rec({ eventHash: "b", title: "💇 Haircut", startDate: "2026-08-20" });

    expect(findEvent("Haircut", "2026-09-01", [a, b])).toBeUndefined();
  });

  it("ignores cancelled candidates", () => {
    const cancelled = rec({
      eventHash: "c",
      title: "💇 Haircut",
      startDate: "2026-07-20",
      status: "cancelled",
    });

    expect(findEvent("Haircut", "2026-07-20", [cancelled])).toBeUndefined();
  });
});

describe("resolveExplicitEventReference", () => {
  const target = rec({ eventHash: "t" });
  const byId = new Map([["evt_3", target]]);

  it("resolves a bare or bracketed handle", () => {
    expect(resolveExplicitEventReference({ eventId: "evt_3" }, byId)).toBe(target);
    expect(resolveExplicitEventReference({ eventId: "[evt_3]" }, byId)).toBe(target);
  });

  it("returns undefined without an eventId (no title fallback for cancels)", () => {
    expect(resolveExplicitEventReference({}, byId)).toBeUndefined();
    expect(resolveExplicitEventReference({ eventId: undefined }, byId)).toBeUndefined();
  });

  it("returns undefined for an unknown handle", () => {
    expect(resolveExplicitEventReference({ eventId: "evt_99" }, byId)).toBeUndefined();
  });
});

describe("pickByStartDate", () => {
  const a = rec({ eventHash: "a", startDate: "2026-06-17" });
  const b = rec({ eventHash: "b", startDate: "2026-09-01" });

  it("returns the exact startDate match when present", () => {
    expect(pickByStartDate([a, b], "2026-09-01")).toBe(b);
  });

  it("returns a lone candidate when no date matches", () => {
    expect(pickByStartDate([a], "2099-01-01")).toBe(a);
  });

  it("fails closed (undefined) when several share the title and none match the date", () => {
    expect(pickByStartDate([a, b], "2099-01-01")).toBeUndefined();
  });

  it("returns undefined for no candidates", () => {
    expect(pickByStartDate([], "2026-06-17")).toBeUndefined();
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

  it("defaults the fallback to title+date search over the windowed byId values", () => {
    const target = rec({
      eventHash: "windowed",
      title: "💇 Haircut",
      startDate: "2026-07-20",
    });
    const byId = new Map([["evt_1", target]]);

    expect(
      resolveEventReference({ title: "Haircut", startDate: "2026-07-20" }, byId),
    ).toBe(target);
    expect(
      resolveEventReference({ title: "Unrelated", startDate: "2026-07-20" }, byId),
    ).toBeUndefined();
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
