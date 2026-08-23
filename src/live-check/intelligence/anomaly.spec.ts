import { describe, expect, it } from "vitest";
import type { Streamer } from "../streamers.js";
import { computeRelevance, ViewerAnomalyTracker } from "./anomaly.js";

const streamer: Streamer = {
  id: "hutch",
  displayName: "Hutch",
  bindings: [],
  tier: "background",
  dgg: { hosted: true, viewers: 45 },
};

describe("ViewerAnomalyTracker", () => {
  it("flags a sustained viewer surge against older samples", () => {
    const tracker = new ViewerAnomalyTracker();
    tracker.observe({ streamerId: "hutch", viewers: 200, dggViewers: 30, now: 0 });
    tracker.observe({
      streamerId: "hutch",
      viewers: 220,
      dggViewers: 32,
      now: 60_000,
    });
    const trend = tracker.observe({
      streamerId: "hutch",
      viewers: 430,
      dggViewers: 70,
      now: 5 * 60_000,
    });
    expect(trend.anomalous).toBe(true);
    expect(trend.reason).toContain("viewers up");
    expect(trend.reason).toContain("DGG audience up");
  });

  it("ignores ordinary movement and insufficient history", () => {
    const tracker = new ViewerAnomalyTracker();
    expect(
      tracker.observe({ streamerId: "hutch", viewers: 200, dggViewers: 30, now: 0 })
        .anomalous,
    ).toBe(false);
    tracker.observe({
      streamerId: "hutch",
      viewers: 210,
      dggViewers: 31,
      now: 60_000,
    });
    expect(
      tracker.observe({
        streamerId: "hutch",
        viewers: 250,
        dggViewers: 35,
        now: 5 * 60_000,
      }).anomalous,
    ).toBe(false);
  });

  it("does not turn missing platform viewer data into a synthetic zero", () => {
    const tracker = new ViewerAnomalyTracker();
    tracker.observe({ streamerId: "hutch", viewers: 200, dggViewers: null, now: 0 });
    tracker.observe({
      streamerId: "hutch",
      viewers: null,
      dggViewers: null,
      now: 60_000,
    });
    const trend = tracker.observe({
      streamerId: "hutch",
      viewers: 220,
      dggViewers: null,
      now: 5 * 60_000,
    });
    expect(trend.anomalous).toBe(false);
    expect(trend.percentChange).toBe(10);
  });
});

describe("computeRelevance", () => {
  it("makes confirmed Destiny presence decisive", () => {
    const result = computeRelevance({ streamer, destinyConfirmed: true });
    expect(result.score).toBeGreaterThanOrEqual(60);
    expect(result.reasons).toContain("Destiny detected as a live participant");
  });

  it("combines semantic importance, audience, and anomaly", () => {
    const result = computeRelevance({
      streamer,
      semantic: {
        headline: "A live debate is beginning",
        topics: ["debate"],
        contentKind: "debate",
        importance: 90,
        reason: "substantive debate",
        updatedAt: 1,
      },
      trend: {
        percentChange: 70,
        viewersPerMinute: 20,
        dggPercentChange: 120,
        anomalous: true,
        reason: "viewers up 70%",
        updatedAt: 1,
      },
      destinyConfirmed: false,
    });
    expect(result.score).toBeGreaterThanOrEqual(80);
    expect(result.reasons).toContain("substantive debate");
  });
});
