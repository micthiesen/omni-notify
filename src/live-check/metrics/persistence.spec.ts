import { describe, expect, it } from "@effect/vitest";
import { Effect, Exit } from "effect";
import { afterEach, vi } from "vitest";
import { runTest } from "../testRuntime.js";
import { Platform } from "../platforms/index.js";
import {
  getPlatformViewerMetrics,
  PlatformViewerMetricsEntity,
  recordPlatformViewerCount,
  recordPlatformViewerCountEffect,
} from "./persistence.js";

afterEach(() => runTest(PlatformViewerMetricsEntity.deleteAll()));

describe("platform viewer metrics", () => {
  it("keeps each platform account in a separate daily series", async () => {
    const now = new Date("2026-08-25T20:00:00Z");
    await runTest(
      recordPlatformViewerCount({
        streamerId: "iri",
        platform: Platform.YouTube,
        username: "@imreallyimportant",
        viewerCount: 427,
        now,
      }),
    );
    await runTest(
      recordPlatformViewerCount({
        streamerId: "iri",
        platform: Platform.Kick,
        username: "imreallyimportant",
        viewerCount: 475,
        now,
      }),
    );
    await runTest(
      recordPlatformViewerCount({
        streamerId: "iri",
        platform: Platform.Kick,
        username: "IMREALLYIMPORTANT",
        viewerCount: 500,
        now,
      }),
    );

    const metrics = await runTest(getPlatformViewerMetrics("iri"));
    expect(metrics).toHaveLength(2);
    expect(metrics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          platform: Platform.YouTube,
          allTimeMax: 427,
          dailyBuckets: [expect.objectContaining({ maxViewers: 427 })],
        }),
        expect.objectContaining({
          platform: Platform.Kick,
          username: "imreallyimportant",
          allTimeMax: 500,
          dailyBuckets: [expect.objectContaining({ maxViewers: 500 })],
        }),
      ]),
    );
  });

  it("maps an Entity write failure to PersistenceError", async () => {
    vi.spyOn(PlatformViewerMetricsEntity, "upsert").mockReturnValueOnce(
      Effect.fail(new Error("metrics database unavailable")) as never,
    );

    const exit = await runTest(
      Effect.exit(
        recordPlatformViewerCountEffect({
          streamerId: "iri",
          platform: Platform.Kick,
          username: "iri",
          viewerCount: 10,
          now: new Date(0),
        }),
      ),
    );

    expect(Exit.isFailure(exit)).toBe(true);
    if (Exit.isFailure(exit)) {
      expect(String(exit.cause)).toContain("PersistenceError");
      expect(String(exit.cause)).toContain("metrics database unavailable");
    }
  });
});
