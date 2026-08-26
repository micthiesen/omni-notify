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
  const observeStableBaseline = (tracker: ViewerAnomalyTracker) => {
    for (let minute = 0; minute < 15; minute += 1) {
      tracker.observe({
        streamerId: "hutch",
        viewers: 200,
        dggViewers: 30,
        sessionStartedAt: 0,
        now: minute * 60_000,
      });
    }
  };

  it("suppresses the normal audience ramp during the first fifteen minutes", () => {
    const tracker = new ViewerAnomalyTracker();
    for (let minute = 0; minute < 15; minute += 1) {
      const trend = tracker.observe({
        streamerId: "hutch",
        viewers: minute === 0 ? 1 : 400,
        dggViewers: minute === 0 ? 2 : 100,
        sessionStartedAt: 0,
        now: minute * 60_000,
      });
      expect(trend.anomalous).toBe(false);
      expect(trend.suppressionReason).toContain("Building a post-start baseline");
    }
    const mature = tracker.observe({
      streamerId: "hutch",
      viewers: 400,
      dggViewers: 100,
      sessionStartedAt: 0,
      now: 15 * 60_000,
    });
    expect(mature.anomalous).toBe(false);
  });

  it("flags a sustained late viewer surge against a mature baseline", () => {
    const tracker = new ViewerAnomalyTracker();
    observeStableBaseline(tracker);
    const candidate = tracker.observe({
      streamerId: "hutch",
      viewers: 430,
      dggViewers: 70,
      sessionStartedAt: 0,
      now: 16 * 60_000,
    });
    expect(candidate.anomalous).toBe(false);
    expect(candidate.suppressionReason).toContain("another observation");
    const trend = tracker.observe({
      streamerId: "hutch",
      viewers: 440,
      dggViewers: 72,
      sessionStartedAt: 0,
      now: 17 * 60_000,
    });
    expect(trend.anomalous).toBe(true);
    expect(trend.reason).toContain("viewers up");
    expect(trend.reason).toContain("200 baseline");
    expect(trend.reason).toContain("DGG audience up");
  });

  it("does not confirm a one-observation scrape spike", () => {
    const tracker = new ViewerAnomalyTracker();
    observeStableBaseline(tracker);
    const sample = (viewers: number, minute: number) =>
      tracker.observe({
        streamerId: "hutch",
        viewers,
        dggViewers: 30,
        sessionStartedAt: 0,
        now: minute * 60_000,
      });
    expect(sample(430, 16).anomalous).toBe(false);
    expect(sample(205, 17).anomalous).toBe(false);
    expect(sample(430, 18).anomalous).toBe(false);
  });

  it("does not combine unrelated platform and DGG spikes into confirmation", () => {
    const tracker = new ViewerAnomalyTracker();
    observeStableBaseline(tracker);
    expect(
      tracker.observe({
        streamerId: "hutch",
        viewers: 430,
        dggViewers: 30,
        sessionStartedAt: 0,
        now: 16 * 60_000,
      }).anomalous,
    ).toBe(false);
    expect(
      tracker.observe({
        streamerId: "hutch",
        viewers: 200,
        dggViewers: 70,
        sessionStartedAt: 0,
        now: 17 * 60_000,
      }).anomalous,
    ).toBe(false);
  });

  it("retains a PRSEK-shaped late seventy-one-percent surge", () => {
    const tracker = new ViewerAnomalyTracker();
    for (let minute = 0; minute < 15; minute += 1) {
      tracker.observe({
        streamerId: "prsek",
        viewers: 150,
        dggViewers: 30,
        sessionStartedAt: 0,
        now: minute * 60_000,
      });
    }
    tracker.observe({
      streamerId: "prsek",
      viewers: 250,
      dggViewers: 30,
      sessionStartedAt: 0,
      now: 16 * 60_000,
    });
    const trend = tracker.observe({
      streamerId: "prsek",
      viewers: 256,
      dggViewers: 30,
      sessionStartedAt: 0,
      now: 17 * 60_000,
    });
    expect(trend.anomalous).toBe(true);
    expect(trend.percentChange).toBeCloseTo(70.67, 2);
  });

  it("does not turn missing platform viewer data into a synthetic zero", () => {
    const tracker = new ViewerAnomalyTracker();
    for (let minute = 0; minute < 15; minute += 1) {
      tracker.observe({
        streamerId: "hutch",
        viewers: minute === 5 ? null : 200,
        dggViewers: null,
        sessionStartedAt: 0,
        now: minute * 60_000,
      });
    }
    const trend = tracker.observe({
      streamerId: "hutch",
      viewers: 220,
      dggViewers: null,
      sessionStartedAt: 0,
      now: 16 * 60_000,
    });
    expect(trend.anomalous).toBe(false);
    expect(trend.percentChange).toBe(10);
  });

  it("clears confirmation evidence between sessions", () => {
    const tracker = new ViewerAnomalyTracker();
    observeStableBaseline(tracker);
    tracker.observe({
      streamerId: "hutch",
      viewers: 430,
      dggViewers: 30,
      sessionStartedAt: 0,
      now: 16 * 60_000,
    });
    tracker.clear("hutch");
    const nextSession = tracker.observe({
      streamerId: "hutch",
      viewers: 430,
      dggViewers: 30,
      sessionStartedAt: 17 * 60_000,
      now: 17 * 60_000,
    });
    expect(nextSession.anomalous).toBe(false);
    expect(nextSession.candidateObservations).toBe(0);
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
