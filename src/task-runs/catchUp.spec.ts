import { describe, expect, it } from "vitest";
import { decideCatchUp } from "./catchUp.js";

const HOUR_MS = 60 * 60 * 1000;

function localTime(day: number, hour: number, minute = 0): number {
  return new Date(2026, 6, day, hour, minute).getTime();
}

describe("decideCatchUp", () => {
  it.each([
    ["minutely", "0 * * * * *"],
    ["hourly", "0 0 * * * *"],
  ])("does not recover %s schedules", (_name, schedule) => {
    const now = localTime(15, 10, 30);
    expect(decideCatchUp(schedule, now - 2 * HOUR_MS, now).kind).toBe("disabled");
  });

  it("recovers a daily task up to six hours late", () => {
    const now = localTime(15, 10);
    const decision = decideCatchUp("0 0 5 * * *", localTime(14, 6), now);

    expect(decision).toEqual({
      kind: "run",
      scheduledFor: localTime(15, 5),
      latenessMs: 5 * HOUR_MS,
      maxLatenessMs: 6 * HOUR_MS,
    });
  });

  it("skips a daily task more than six hours late", () => {
    const decision = decideCatchUp("0 0 5 * * *", localTime(14, 6), localTime(15, 12));

    expect(decision.kind).toBe("stale");
  });

  it("uses a twelve-hour window for a Mon/Wed/Fri task", () => {
    const decision = decideCatchUp(
      "0 0 5 * * 1,3,5",
      localTime(15, 6),
      localTime(17, 12),
    );

    expect(decision).toMatchObject({
      kind: "run",
      scheduledFor: localTime(17, 5),
      latenessMs: 7 * HOUR_MS,
      maxLatenessMs: 12 * HOUR_MS,
    });
  });

  it("recovers a weekly task within its 42-hour window", () => {
    const decision = decideCatchUp("0 0 4 * * 0", localTime(5, 5), localTime(13, 16));

    expect(decision).toMatchObject({
      kind: "run",
      scheduledFor: localTime(12, 4),
      latenessMs: 36 * HOUR_MS,
      maxLatenessMs: 42 * HOUR_MS,
    });
  });

  it("caps the catch-up window at 48 hours", () => {
    const withinWindow = decideCatchUp(
      "0 0 0 1 * *",
      localTime(1, 1),
      new Date(2026, 7, 2, 23).getTime(),
    );
    const outsideWindow = decideCatchUp(
      "0 0 0 1 * *",
      localTime(1, 1),
      new Date(2026, 7, 3, 1).getTime(),
    );

    expect(withinWindow).toMatchObject({
      kind: "run",
      maxLatenessMs: 48 * HOUR_MS,
    });
    expect(outsideWindow).toMatchObject({
      kind: "stale",
      maxLatenessMs: 48 * HOUR_MS,
    });
  });

  it("does nothing when the newest occurrence was already evaluated", () => {
    const now = localTime(15, 10);
    expect(decideCatchUp("0 0 5 * * *", localTime(15, 5), now)).toEqual({
      kind: "none",
    });
  });
});
