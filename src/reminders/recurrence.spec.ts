import { describe, expect, it } from "vitest";
import { decodeRecurrenceValues, encodeRecurrenceValues } from "./recurrence.js";

const bytes = (value: unknown) => Buffer.from(JSON.stringify(value)).toString("base64");

describe("Apple recurrence service values", () => {
  it.each(["daily", "weekly", "monthly", "yearly", "hourly", "minutely", "secondly"])(
    "round trips %s with Apple's zero-based frequency",
    (frequency) => {
      const order = [
        "daily",
        "weekly",
        "monthly",
        "yearly",
        "hourly",
        "minutely",
        "secondly",
      ];
      const encoded = encodeRecurrenceValues({ frequency, interval: 1 });
      expect(encoded).toEqual({
        supported: true,
        values: { Frequency: order.indexOf(frequency), Interval: 1 },
      });
      if (encoded.supported)
        expect(decodeRecurrenceValues(encoded.values)).toEqual({
          supported: true,
          rule: { frequency, interval: 1 },
        });
    },
  );

  it("round trips every selector without changing signed values, absent fields, nulls or weekday ordinals", () => {
    const values = {
      Frequency: 3,
      Interval: 2,
      OccurrenceCount: 0,
      FirstDayOfTheWeek: 2,
      EndDate: 1_800_000_000_000,
      DaysOfTheWeek: bytes([{ dayOfTheWeek: 2, weekNumber: 0 }, { dayOfTheWeek: 7 }]),
      DaysOfTheMonth: bytes([1, -1]),
      DaysOfTheYear: bytes([1, -366]),
      WeeksOfTheYear: bytes([1, -53]),
      MonthsOfTheYear: bytes([1, 12]),
      SetPositions: bytes([1, -1]),
    };
    const decoded = decodeRecurrenceValues(values);
    expect(decoded.supported).toBe(true);
    if (decoded.supported)
      expect(encodeRecurrenceValues(decoded.rule)).toEqual({ supported: true, values });
    expect(
      decodeRecurrenceValues({ Frequency: 0, Interval: 1, DaysOfTheWeek: null }),
    ).toEqual({
      supported: true,
      rule: { frequency: "daily", interval: 1, daysOfWeek: null },
    });
    expect(
      encodeRecurrenceValues({
        frequency: "monthly",
        interval: 1,
        daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: -1 }],
      }),
    ).toEqual({
      supported: true,
      values: {
        Frequency: 2,
        Interval: 1,
        DaysOfTheWeek: bytes([{ dayOfTheWeek: 2, weekNumber: -1 }]),
      },
    });
  });

  it.each([-1, 7, 1.5, "0", null, undefined])(
    "keeps unknown frequency %j unsupported without a daily fallback",
    (Frequency) => {
      expect(decodeRecurrenceValues({ Frequency, Interval: 1 })).toMatchObject({
        supported: false,
        reason: "unknown_frequency",
      });
    },
  );

  it("does not infer absent defaults or silently discard unknown fields", () => {
    expect(decodeRecurrenceValues({ Frequency: 0 })).toMatchObject({
      supported: false,
    });
    expect(
      decodeRecurrenceValues({ Frequency: 0, Interval: 1, NewRule: "private-data" }),
    ).toEqual({ supported: false, reason: "unknown_fields", fields: ["NewRule"] });
    expect(
      encodeRecurrenceValues({ frequency: "daily", interval: 1, newRule: true }),
    ).toMatchObject({ supported: false });
    expect(
      decodeRecurrenceValues({
        Frequency: 0,
        Interval: 1,
        DaysOfTheWeek: bytes([{ dayOfTheWeek: 2, unknown: true }]),
      }),
    ).toMatchObject({ supported: false });
  });

  it.each([
    "%%%",
    "W10",
    "W11=",
    bytes("not an array"),
    bytes({}),
    Buffer.from("not json").toString("base64"),
    "A".repeat(30_000),
  ])("rejects malformed or oversized selector without exposing it", (DaysOfTheWeek) => {
    const result = decodeRecurrenceValues({ Frequency: 0, Interval: 1, DaysOfTheWeek });
    expect(result.supported).toBe(false);
    expect(JSON.stringify(result)).not.toContain(DaysOfTheWeek);
  });

  it.each([
    { interval: 0 },
    { interval: 1.5 },
    { occurrenceCount: -1 },
    { endDate: -1 },
    { endDate: Infinity },
    { firstDayOfWeek: -1 },
    { firstDayOfWeek: 8 },
    { daysOfWeek: [{ dayOfTheWeek: 0 }] },
    { daysOfWeek: [{ dayOfTheWeek: 8 }] },
    { daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: 54 }] },
    { daysOfMonth: [0] },
    { daysOfMonth: [32] },
    { daysOfYear: [-367] },
    { weeksOfYear: [54] },
    { monthsOfYear: [13] },
    { setPositions: [0] },
  ])("rejects out-of-range values %j", (patch) => {
    expect(
      encodeRecurrenceValues({ frequency: "yearly", interval: 1, ...patch }),
    ).toMatchObject({ supported: false });
  });

  it.each([
    { frequency: "weekly", daysOfMonth: [1] },
    { frequency: "monthly", daysOfYear: [1] },
    { frequency: "daily", weeksOfYear: [1] },
    { frequency: "weekly", daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: 1 }] },
    {
      frequency: "yearly",
      weeksOfYear: [1],
      daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: 1 }],
    },
    { setPositions: [1] },
    { occurrenceCount: 3, endDate: 1_800_000_000_000 },
  ])("rejects incompatible selectors %j", (patch) => {
    expect(
      encodeRecurrenceValues({ frequency: "daily", interval: 1, ...patch }),
    ).toMatchObject({ supported: false });
  });

  it("handles non-object inputs without throwing", () => {
    for (const value of [null, 4, "rule", [], false]) {
      expect(decodeRecurrenceValues(value).supported).toBe(false);
      expect(encodeRecurrenceValues(value).supported).toBe(false);
    }
  });

  it("preserves the observed server first-day value zero without interpreting it as Sunday", () => {
    const values = { Frequency: 0, Interval: 1, FirstDayOfTheWeek: 0 };
    const decoded = decodeRecurrenceValues(values);
    expect(decoded).toEqual({
      supported: true,
      rule: { frequency: "daily", interval: 1, firstDayOfWeek: 0 },
    });
    if (decoded.supported)
      expect(encodeRecurrenceValues(decoded.rule)).toEqual({ supported: true, values });
  });
});
