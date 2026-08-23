import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it } from "vitest";
import {
  buildLivestreamFeedbackDigest,
  LivestreamFeedbackEntity,
  LivestreamIntelligenceEntity,
  recordLivestreamFeedback,
  saveLivestreamIntelligence,
} from "./persistence.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.ERROR,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: `/tmp/omni-livestream-intelligence-${process.pid}.db`,
  },
});

afterEach(() => {
  LivestreamFeedbackEntity.deleteAll();
  LivestreamIntelligenceEntity.deleteAll();
});

describe("livestream alert feedback", () => {
  it("records feedback only for the streamer's latest alert", () => {
    saveLivestreamIntelligence({
      streamerId: "hutch",
      sessionStartedAt: 1,
      relevanceScore: 80,
      relevanceReasons: [],
      chapters: [],
      latestAlert: {
        alertId: "alert-1",
        type: "debate",
        title: "Debate starting",
        message: "A substantive debate is beginning.",
        reason: "Live disagreement with turn-taking",
        confidence: 0.9,
        createdAt: 2,
      },
      updatedAt: 2,
    });
    expect(
      recordLivestreamFeedback({
        streamerId: "hutch",
        alertId: "stale-alert",
        verdict: "false_positive",
      }),
    ).toBeUndefined();
    const feedback = recordLivestreamFeedback({
      streamerId: "hutch",
      alertId: "alert-1",
      verdict: "useful",
      note: "  exactly what I wanted  ",
    });
    expect(feedback).toMatchObject({
      streamerId: "hutch",
      alertType: "debate",
      verdict: "useful",
      note: "exactly what I wanted",
    });
    expect(buildLivestreamFeedbackDigest()).toBe(
      "debate: useful (exactly what I wanted)",
    );
    recordLivestreamFeedback({
      streamerId: "hutch",
      alertId: "alert-1",
      verdict: "not_useful",
    });
    expect(LivestreamFeedbackEntity.getAll()).toHaveLength(1);
    expect(buildLivestreamFeedbackDigest()).toBe("debate: not_useful");
  });
});
