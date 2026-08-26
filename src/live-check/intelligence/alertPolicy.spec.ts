import { describe, expect, it } from "vitest";
import { alertSentInSession, livestreamAlertConfidenceFloor } from "./alertPolicy.js";

describe("livestreamAlertConfidenceFloor", () => {
  it("does not reapply the generic semantic floor to a confirmed Destiny guest", () => {
    const floor = livestreamAlertConfidenceFloor({
      type: "destiny_guest",
      feedbackDigest: "",
      destinySpeakerThreshold: 0.62,
    });
    expect(floor).toBe(0.62);
    expect(0.7059429831388251).toBeGreaterThanOrEqual(floor);
  });

  it("keeps the ordinary semantic alert floor", () => {
    expect(
      livestreamAlertConfidenceFloor({
        type: "debate",
        feedbackDigest: "",
        destinySpeakerThreshold: 0.62,
      }),
    ).toBe(0.75);
  });

  it("raises ordinary alert confidence after repeated negative feedback", () => {
    expect(
      livestreamAlertConfidenceFloor({
        type: "debate",
        feedbackDigest: "debate:not_useful\ndebate:false_positive",
        destinySpeakerThreshold: 0.62,
      }),
    ).toBe(0.9);
  });

  it("raises only the dedicated Destiny floor after negative feedback", () => {
    expect(
      livestreamAlertConfidenceFloor({
        type: "destiny_guest",
        feedbackDigest: "destiny_guest:not_useful\ndestiny_guest:false_positive",
        destinySpeakerThreshold: 0.62,
      }),
    ).toBe(0.75);
  });
});

describe("alertSentInSession", () => {
  const state = {
    streamerId: "darius",
    sessionStartedAt: 1_000,
    relevanceScore: 0,
    relevanceReasons: [],
    chapters: [],
    updatedAt: 2_000,
  };

  it("retains per-type dedup when another alert becomes latest", () => {
    expect(
      alertSentInSession(
        {
          ...state,
          alertedAtByType: { destiny_guest: 1_500 },
          latestAlert: {
            alertId: "later",
            type: "debate",
            title: "Debate",
            message: "A debate started",
            reason: "Transcript evidence",
            confidence: 0.9,
            createdAt: 1_800,
          },
        },
        "destiny_guest",
      ),
    ).toBe(true);
  });

  it("does not carry dedup into a newer session", () => {
    expect(
      alertSentInSession(
        {
          ...state,
          sessionStartedAt: 2_000,
          alertedAtByType: { destiny_guest: 1_500 },
        },
        "destiny_guest",
      ),
    ).toBe(false);
  });

  it("durably deduplicates viewer surges by type within the session", () => {
    expect(
      alertSentInSession(
        {
          ...state,
          alertedAtByType: { viewer_surge: 1_500 },
          latestAlert: {
            alertId: "later",
            type: "destiny_guest",
            title: "Destiny is present",
            message: "Confirmed participant",
            reason: "Voice evidence",
            confidence: 0.8,
            createdAt: 1_800,
          },
        },
        "viewer_surge",
      ),
    ).toBe(true);
  });
});
