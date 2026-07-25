import { describe, expect, it } from "vitest";
import { OutageAlerter, type UnknownStreak } from "./outage.js";

const MINUTE = 60_000;

function streak(
  displayName: string,
  ticks: number,
  error = "Timeout awaiting 'request' for 10000ms",
): UnknownStreak {
  return { displayName, ticks, error };
}

/** Feed the alerter N clean ticks, returning the alerts it produced. */
function clean(alerter: OutageAlerter, count: number, startMs: number) {
  const alerts = [];
  for (let i = 0; i < count; i++) {
    alerts.push(alerter.evaluate([], 4, startMs + i * 20_000));
  }
  return alerts;
}

describe("OutageAlerter", () => {
  it("stays quiet below the tick threshold", () => {
    const alerter = new OutageAlerter();
    expect(alerter.evaluate([streak("Radiant", 1)], 4, 0)).toBeNull();
    expect(alerter.evaluate([streak("Radiant", 2)], 4, 20_000)).toBeNull();
  });

  it("alerts once when the outage is confirmed", () => {
    const alerter = new OutageAlerter();
    const alert = alerter.evaluate(
      [streak("Radiant", 3), streak("AnythingElse", 3)],
      4,
      0,
    );
    expect(alert).toEqual({
      kind: "degraded",
      title: "Live check degraded: 2/4 streamers unreachable",
      message: "Radiant, AnythingElse\nTimeout awaiting 'request' for 10000ms",
    });
  });

  it("does not re-alert on every subsequent tick", () => {
    const alerter = new OutageAlerter();
    alerter.evaluate([streak("Radiant", 3)], 4, 0);
    expect(alerter.evaluate([streak("Radiant", 4)], 4, 20_000)).toBeNull();
    expect(alerter.evaluate([streak("Radiant", 20)], 4, 10 * MINUTE)).toBeNull();
  });

  it("escalates on a widening schedule while the outage persists", () => {
    const alerter = new OutageAlerter();
    alerter.evaluate([streak("Radiant", 3)], 4, 0);
    const second = alerter.evaluate([streak("Radiant", 100)], 4, 30 * MINUTE);
    expect(second?.kind).toBe("degraded");
    expect(second?.message).toContain("Unreachable for 30m.");
    // Next reminder is two hours out, not another thirty minutes.
    expect(alerter.evaluate([streak("Radiant", 200)], 4, 60 * MINUTE)).toBeNull();
    expect(alerter.evaluate([streak("Radiant", 400)], 4, 150 * MINUTE)?.kind).toBe(
      "degraded",
    );
  });

  it("sends one recovery notice once the fleet stays clean", () => {
    const alerter = new OutageAlerter();
    alerter.evaluate([streak("Radiant", 3)], 4, 0);
    const [first, second, third] = clean(alerter, 3, 12 * MINUTE);
    expect(first).toBeNull();
    expect(second).toBeNull();
    expect(third).toEqual({
      kind: "recovered",
      title: "Live check recovered",
      message: "All streamers reachable again after 13m.",
    });
    expect(alerter.evaluate([], 4, 20 * MINUTE)).toBeNull();
  });

  it("stays silent for a blip that never reached the threshold", () => {
    const alerter = new OutageAlerter();
    alerter.evaluate([streak("Radiant", 2)], 4, 0);
    expect(clean(alerter, 3, MINUTE)).toEqual([null, null, null]);
  });

  it("does not emit degraded/recovered pairs for a flapping streamer", () => {
    const alerter = new OutageAlerter();
    expect(alerter.evaluate([streak("Radiant", 3)], 4, 0)?.kind).toBe("degraded");
    // One good tick, then failing again: still the same outage, no new alerts.
    for (let cycle = 1; cycle <= 5; cycle++) {
      const base = cycle * MINUTE;
      expect(alerter.evaluate([], 4, base)).toBeNull();
      expect(alerter.evaluate([streak("Radiant", 3)], 4, base + 20_000)).toBeNull();
      expect(alerter.evaluate([streak("Radiant", 4)], 4, base + 40_000)).toBeNull();
    }
  });

  it("restarts escalation for a genuinely new outage after recovery", () => {
    const alerter = new OutageAlerter();
    alerter.evaluate([streak("Radiant", 3)], 4, 0);
    clean(alerter, 3, MINUTE);
    expect(alerter.evaluate([streak("Radiant", 3)], 4, 5 * MINUTE)?.kind).toBe(
      "degraded",
    );
  });

  it("summarizes large outages with a name tail and distinct errors", () => {
    const alerter = new OutageAlerter();
    const streaks = [
      streak("A", 9),
      streak("B", 8),
      streak("C", 7),
      streak("D", 6),
      streak("E", 5),
      streak("F", 4, "ECONNREFUSED"),
      streak("G", 3, "403 Forbidden"),
    ];
    const alert = alerter.evaluate(streaks, 7, 0);
    expect(alert?.title).toBe("Live check degraded: 7/7 streamers unreachable");
    expect(alert?.message).toBe(
      [
        "A, B, C, D, E +2 more",
        "Timeout awaiting 'request' for 10000ms",
        "ECONNREFUSED",
        "+1 other error(s)",
      ].join("\n"),
    );
  });
});
