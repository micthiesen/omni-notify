import { afterEach, describe, expect, it } from "vitest";
import { runTest } from "../testRuntime.js";
import {
  buildLivestreamFeedbackDigest,
  DESTINY_CONFIRMED_EVENT_TITLE,
  getLatestDestinyConfirmation,
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

afterEach(async () => {
  await runTest(LivestreamFeedbackEntity.deleteAll());
  await runTest(LivestreamDiagnosticsEntity.deleteAll());
  await runTest(LivestreamIntelligenceEventEntity.deleteAll());
  await runTest(LivestreamIntelligenceEntity.deleteAll());
});

describe("livestream alert feedback", () => {
  it("records feedback only for the latest alert", async () => {
    await runTest(
      saveLivestreamIntelligence({
        streamerId: "hutch",
        sessionStartedAt: 1,
        relevanceScore: 80,
        relevanceReasons: [],
        chapters: [],
        latestAlert: {
          alertId: "alert-1",
          type: "debate",
          title: "Debate",
          message: "Starting",
          reason: "Evidence",
          confidence: 0.9,
          createdAt: 2,
        },
        updatedAt: 2,
      }),
    );
    expect(
      await runTest(
        recordLivestreamFeedback({
          streamerId: "hutch",
          alertId: "stale",
          verdict: "false_positive",
        }),
      ),
    ).toBeUndefined();
    expect(
      await runTest(
        recordLivestreamFeedback({
          streamerId: "hutch",
          alertId: "alert-1",
          verdict: "useful",
          note: "  exactly what I wanted  ",
        }),
      ),
    ).toMatchObject({ note: "exactly what I wanted" });
    expect(await runTest(buildLivestreamFeedbackDigest())).toBe(
      "debate: useful (exactly what I wanted)",
    );
  });
});

describe("livestream pipeline diagnostics", () => {
  it("merges stages within a session and resets for a new session", async () => {
    await runTest(
      updateLivestreamStage("pisco", 100, "metadata", {
        status: "success",
        detail: "Politics",
      }),
    );
    await runTest(
      updateLivestreamStage("pisco", 100, "summary", {
        status: "running",
        startedAt: 120,
      }),
    );
    expect((await runTest(getLivestreamDiagnostics("pisco")))?.stages).toMatchObject({
      metadata: { status: "success" },
      summary: { status: "running" },
    });
    await runTest(
      updateLivestreamStage("pisco", 200, "voice", { status: "idle", eligible: false }),
    );
    expect(await runTest(getLivestreamDiagnostics("pisco"))).toMatchObject({
      sessionStartedAt: 200,
      stages: { voice: { status: "idle" } },
    });
  });

  it("returns a bounded timeline and durable confirmation", async () => {
    await runTest(
      recordLivestreamEvent({
        streamerId: "darius",
        sessionStartedAt: 100,
        kind: "voice",
        status: "success",
        title: DESTINY_CONFIRMED_EVENT_TITLE,
        detail: "Live conversation confirmed",
        metrics: { speakerConfidence: 0.706 },
        createdAt: 200,
      }),
    );
    expect(await runTest(getLivestreamEvents("darius", 1))).toHaveLength(1);
    expect(await runTest(getLatestDestinyConfirmation("darius", 100))).toMatchObject({
      detail: "Live conversation confirmed",
    });
    expect(await runTest(getLatestDestinyConfirmation("darius", 300))).toBeUndefined();
  });
});
