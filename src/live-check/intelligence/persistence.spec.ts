import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it } from "vitest";
import {
  buildLivestreamFeedbackDigest,
  getLivestreamDiagnostics,
  getLivestreamEvents,
  LivestreamDiagnosticsEntity,
  LivestreamFeedbackEntity,
  LivestreamIntelligenceEntity,
  LivestreamIntelligenceEventEntity,
  recordLivestreamEvent,
  recordLivestreamFeedback,
  saveLivestreamIntelligence,
  updateLivestreamStage,
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
  LivestreamDiagnosticsEntity.deleteAll();
  LivestreamIntelligenceEventEntity.deleteAll();
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
    expect(getLivestreamEvents("hutch")[0]).toMatchObject({
      kind: "feedback",
      title: "Alert marked not useful",
    });
  });
});

describe("livestream pipeline diagnostics", () => {
  it("merges stages within a session and resets them for a new session", () => {
    updateLivestreamStage("pisco", 100, "metadata", {
      status: "success",
      detail: "Politics",
    });
    updateLivestreamStage("pisco", 100, "summary", {
      status: "running",
      startedAt: 120,
    });
    expect(getLivestreamDiagnostics("pisco")?.stages).toMatchObject({
      metadata: { status: "success" },
      summary: { status: "running" },
    });

    updateLivestreamStage("pisco", 200, "voice", {
      status: "idle",
      eligible: false,
    });
    expect(getLivestreamDiagnostics("pisco")).toMatchObject({
      sessionStartedAt: 200,
      stages: { voice: { status: "idle", eligible: false } },
    });
    expect(getLivestreamDiagnostics("pisco")?.stages.metadata).toBeUndefined();
  });

  it("returns a newest-first bounded per-stream timeline", () => {
    recordLivestreamEvent({
      streamerId: "pisco",
      kind: "metadata",
      status: "success",
      title: "Metadata updated",
      createdAt: 100,
    });
    recordLivestreamEvent({
      streamerId: "hutch",
      kind: "summary",
      status: "success",
      title: "Other stream",
      createdAt: 150,
    });
    recordLivestreamEvent({
      streamerId: "pisco",
      kind: "summary",
      status: "success",
      title: "Summary updated",
      createdAt: 200,
    });
    expect(getLivestreamEvents("pisco", 1).map((event) => event.title)).toEqual([
      "Summary updated",
    ]);
  });
});
