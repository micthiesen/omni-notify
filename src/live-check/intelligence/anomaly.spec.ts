import { describe, expect, it } from "vitest";
import { Platform } from "../platforms/index.js";
import type { StreamSession } from "../sessions.js";
import type { Streamer } from "../streamers.js";
import {
  computeRelevance,
  typicalSessionPeak,
  ViewerAnomalyTracker,
} from "./anomaly.js";

const streamer: Streamer = {
  id: "hutch",
  displayName: "Hutch",
  bindings: [],
  tier: "background",
  dgg: { hosted: true, viewers: 45 },
};

const MINUTE = 60_000;

describe("ViewerAnomalyTracker", () => {
  const observeStableBaseline = (tracker: ViewerAnomalyTracker, minutes = 25) => {
    for (let minute = 0; minute < minutes; minute += 1) {
      tracker.observe({
        streamerId: "hutch",
        viewers: 200,
        dggViewers: 30,
        sessionStartedAt: 0,
        now: minute * MINUTE,
      });
    }
  };

  it("suppresses the normal audience ramp during the first twenty minutes", () => {
    const tracker = new ViewerAnomalyTracker();
    for (let minute = 0; minute < 20; minute += 1) {
      const trend = tracker.observe({
        streamerId: "hutch",
        viewers: minute === 0 ? 1 : 400,
        dggViewers: minute === 0 ? 2 : 100,
        sessionStartedAt: 0,
        now: minute * MINUTE,
      });
      expect(trend.anomalous).toBe(false);
      expect(trend.suppressionReason).toContain("Building a post-start baseline");
    }
  });

  it("never flags a gradual post-start ramp, even after the warmup", () => {
    // Shaped like the go-live ramps that produced every recent false alert:
    // the audience roughly doubles between minutes 10 and 20.
    const tracker = new ViewerAnomalyTracker();
    const reasons = new Set<string | null | undefined>();
    for (let minute = 0; minute <= 90; minute += 1) {
      const viewers = Math.round(1_500 * (1 - Math.exp(-minute / 15)));
      const trend = tracker.observe({
        streamerId: "anythingelse",
        viewers,
        dggViewers: Math.round(viewers / 8),
        sessionStartedAt: 0,
        totalViewers: viewers,
        now: minute * MINUTE,
      });
      expect(trend.anomalous).toBe(false);
      reasons.add(trend.suppressionReason);
    }
    expect(reasons).toContain("Audience is still ramping up");
  });

  it("flags a sudden surge against a flat baseline", () => {
    const tracker = new ViewerAnomalyTracker();
    observeStableBaseline(tracker);
    const candidate = tracker.observe({
      streamerId: "hutch",
      viewers: 430,
      dggViewers: 70,
      sessionStartedAt: 0,
      now: 25 * MINUTE,
    });
    expect(candidate.anomalous).toBe(false);
    expect(candidate.suppressionReason).toContain("another observation");
    const trend = tracker.observe({
      streamerId: "hutch",
      viewers: 440,
      dggViewers: 72,
      sessionStartedAt: 0,
      now: 26 * MINUTE,
    });
    expect(trend.anomalous).toBe(true);
    expect(trend.reason).toContain("viewers up");
    expect(trend.reason).toContain("200 baseline");
    expect(trend.reason).toContain("DGG audience up");
  });

  it("requires a platform surge to reach the typical session peak", () => {
    const observeSurge = (typicalPeak: number) => {
      const tracker = new ViewerAnomalyTracker();
      let trend;
      for (let minute = 0; minute < 27; minute += 1) {
        const viewers = minute < 25 ? 500 : 900;
        trend = tracker.observe({
          streamerId: "anythingelse",
          viewers,
          dggViewers: null,
          sessionStartedAt: 0,
          totalViewers: viewers + 100,
          typicalPeak,
          now: minute * MINUTE,
        });
      }
      return trend;
    };
    const ordinary = observeSurge(2_500);
    expect(ordinary?.anomalous).toBe(false);
    expect(ordinary?.suppressionReason).toBe("Below the typical session peak of 2500");
    expect(ordinary?.typicalPeakViewers).toBe(2_500);
    expect(observeSurge(950)?.anomalous).toBe(true);
  });

  it("restarts the baseline when the primary binding changes", () => {
    const tracker = new ViewerAnomalyTracker();
    for (let minute = 0; minute < 27; minute += 1) {
      const switched = minute >= 25;
      const trend = tracker.observe({
        streamerId: "destiny",
        viewers: switched ? 3_000 : 1_000,
        dggViewers: null,
        sessionStartedAt: 0,
        sourceKey: switched ? "kick:destiny" : "youtube:destiny",
        now: minute * MINUTE,
      });
      expect(trend.anomalous).toBe(false);
    }
  });

  it("does not trust a short post-restart history to rule out a ramp", () => {
    // A restart at minute 22 leaves only samples newer than the window midpoint.
    const tracker = new ViewerAnomalyTracker();
    const tick = MINUTE / 3;
    for (let at = 22 * MINUTE; at < 32 * MINUTE; at += tick) {
      const trend = tracker.observe({
        streamerId: "anythingelse",
        viewers: at < 24.5 * MINUTE ? 1_000 : 1_650,
        dggViewers: null,
        sessionStartedAt: 0,
        now: at,
      });
      expect(trend.anomalous).toBe(false);
    }
  });

  it("keeps the DGG baseline across a primary switch", () => {
    const tracker = new ViewerAnomalyTracker();
    let trend;
    for (let minute = 0; minute < 27; minute += 1) {
      trend = tracker.observe({
        streamerId: "hutch",
        viewers: 200,
        dggViewers: minute < 25 ? 30 : 90,
        sessionStartedAt: 0,
        sourceKey: minute < 24 ? "youtube:hutch" : "kick:hutch",
        now: minute * MINUTE,
      });
    }
    expect(trend?.anomalous).toBe(true);
    expect(trend?.reason).toContain("DGG audience up");
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
        now: minute * MINUTE,
      });
    expect(sample(430, 25).anomalous).toBe(false);
    expect(sample(205, 26).anomalous).toBe(false);
    expect(sample(430, 27).anomalous).toBe(false);
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
        now: 25 * MINUTE,
      }).anomalous,
    ).toBe(false);
    expect(
      tracker.observe({
        streamerId: "hutch",
        viewers: 200,
        dggViewers: 70,
        sessionStartedAt: 0,
        now: 26 * MINUTE,
      }).anomalous,
    ).toBe(false);
  });

  it("retains a PRSEK-shaped late seventy-one-percent surge", () => {
    const tracker = new ViewerAnomalyTracker();
    for (let minute = 0; minute < 25; minute += 1) {
      tracker.observe({
        streamerId: "prsek",
        viewers: 150,
        dggViewers: 30,
        sessionStartedAt: 0,
        now: minute * MINUTE,
      });
    }
    tracker.observe({
      streamerId: "prsek",
      viewers: 250,
      dggViewers: 30,
      sessionStartedAt: 0,
      now: 25 * MINUTE,
    });
    const trend = tracker.observe({
      streamerId: "prsek",
      viewers: 256,
      dggViewers: 30,
      sessionStartedAt: 0,
      now: 26 * MINUTE,
    });
    expect(trend.anomalous).toBe(true);
    expect(trend.percentChange).toBeCloseTo(70.67, 2);
  });

  it("does not turn missing platform viewer data into a synthetic zero", () => {
    const tracker = new ViewerAnomalyTracker();
    for (let minute = 0; minute < 25; minute += 1) {
      tracker.observe({
        streamerId: "hutch",
        viewers: minute === 10 ? null : 200,
        dggViewers: null,
        sessionStartedAt: 0,
        now: minute * MINUTE,
      });
    }
    const trend = tracker.observe({
      streamerId: "hutch",
      viewers: 220,
      dggViewers: null,
      sessionStartedAt: 0,
      now: 25 * MINUTE,
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
      now: 25 * MINUTE,
    });
    tracker.clear("hutch");
    const nextSession = tracker.observe({
      streamerId: "hutch",
      viewers: 430,
      dggViewers: 30,
      sessionStartedAt: 26 * MINUTE,
      now: 26 * MINUTE,
    });
    expect(nextSession.anomalous).toBe(false);
    expect(nextSession.candidateObservations).toBe(0);
  });
});

describe("typicalSessionPeak", () => {
  const DAY = 24 * 60 * MINUTE;
  const session = (endedAt: number, peakViewers: number, minutes = 120) =>
    ({
      startedAt: endedAt - minutes * MINUTE,
      endedAt,
      durationMs: minutes * MINUTE,
      peakViewers,
      title: "stream",
      platform: Platform.Kick,
      username: "anythingelse",
    }) satisfies StreamSession;
  const now = 100 * DAY;

  it("uses the median peak of recent full sessions", () => {
    expect(
      typicalSessionPeak(
        [
          session(now - 40 * DAY, 9_000),
          session(now - 3 * DAY, 2_400),
          session(now - 2 * DAY, 150, 5),
          session(now - 2 * DAY, 2_600),
          session(now - DAY, 3_600),
        ],
        now,
      ),
    ).toBe(2_600);
  });

  it("returns null without three qualifying sessions", () => {
    expect(
      typicalSessionPeak([session(now - DAY, 2_000), session(now - DAY, 300, 10)], now),
    ).toBeNull();
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
