import { describe, expect, it } from "vitest";
import type { CostEventData } from "./persistence.js";
import { summarizeCosts } from "./summary.js";

function event(
  eventId: string,
  incurredAt: number,
  overrides: Partial<CostEventData> = {},
): CostEventData {
  return {
    eventId,
    incurredAt,
    category: "llm",
    feature: "briefings",
    operation: "generate",
    service: "google",
    model: "gemini-test",
    costCents: 1,
    priceStatus: "estimated",
    usage: { inputTokens: 100, outputTokens: 20, requests: 1 },
    ...overrides,
  };
}

describe("summarizeCosts", () => {
  const now = Date.UTC(2026, 6, 20, 12);

  it("filters the selected range while retaining an all-time total", () => {
    const result = summarizeCosts(
      [
        event("old", now - 40 * 86_400_000, { costCents: 5 }),
        event("recent", now - 2 * 86_400_000, { costCents: 2 }),
        event("future", now + 1, { costCents: 100 }),
      ],
      { days: 30, now, timeZone: "UTC" },
    );

    expect(result.summary.selectedCostCents).toBe(2);
    expect(result.summary.allTimeCostCents).toBe(7);
    expect(result.summary.eventCount).toBe(1);
    expect(result.summary.averageDailyCostCents).toBeCloseTo(2 / 30);
  });

  it("groups fractional costs and preserves unknown usage", () => {
    const result = summarizeCosts(
      [
        event("a", now - 1000, { costCents: 0.25 }),
        event("b", now - 500, {
          category: "retrieval",
          feature: "press-pods",
          service: "jina",
          model: undefined,
          costCents: null,
          priceStatus: "unknown",
          usage: { requests: 1 },
        }),
      ],
      { days: null, now, timeZone: "America/Vancouver" },
    );

    expect(result.summary.selectedCostCents).toBe(0.25);
    expect(result.summary.unknownEventCount).toBe(1);
    expect(result.summary.allTimeUnknownEventCount).toBe(1);
    expect(result.summary.requests).toBe(2);
    expect(result.byFeature).toEqual([
      { feature: "briefings", costCents: 0.25, eventCount: 1, unknownEventCount: 0 },
      { feature: "press-pods", costCents: 0, eventCount: 1, unknownEventCount: 1 },
    ]);
    expect(result.recent[0]).toMatchObject({ eventId: "b", model: null, runId: null });
  });

  it("does not call an unknown-only date the highest priced day", () => {
    const result = summarizeCosts(
      [event("unknown", now, { costCents: null, priceStatus: "unknown" })],
      { days: 7, now, timeZone: "UTC" },
    );

    expect(result.summary.highestDay).toBeNull();
    expect(result.daily[0]).toMatchObject({
      pricedEventCount: 0,
      unknownEventCount: 1,
    });
  });

  it("uses the configured timezone for day boundaries", () => {
    const beforeMidnight = Date.UTC(2026, 6, 20, 6, 59);
    const afterMidnight = Date.UTC(2026, 6, 20, 7, 1);
    const result = summarizeCosts(
      [event("a", beforeMidnight), event("b", afterMidnight)],
      { days: null, now, timeZone: "America/Vancouver" },
    );

    expect(result.daily.map((day) => day.date)).toEqual(["2026-07-19", "2026-07-20"]);
  });
});
