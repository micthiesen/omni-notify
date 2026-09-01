import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { describe, expect, it } from "@effect/vitest";
import { Effect, Exit } from "effect";
import { afterEach, vi } from "vitest";
import { Platform } from "../platforms/index.js";
import {
  getPlatformViewerMetrics,
  PlatformViewerMetricsEntity,
  recordPlatformViewerCount,
  recordPlatformViewerCountEffect,
} from "./persistence.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.ERROR,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: `/tmp/omni-platform-metrics-${process.pid}.db`,
  },
});

afterEach(() => {
  PlatformViewerMetricsEntity.deleteAll();
});

describe("platform viewer metrics", () => {
  it("keeps each platform account in a separate daily series", () => {
    const now = new Date("2026-08-25T20:00:00Z");
    recordPlatformViewerCount({
      streamerId: "iri",
      platform: Platform.YouTube,
      username: "@imreallyimportant",
      viewerCount: 427,
      now,
    });
    recordPlatformViewerCount({
      streamerId: "iri",
      platform: Platform.Kick,
      username: "imreallyimportant",
      viewerCount: 475,
      now,
    });
    recordPlatformViewerCount({
      streamerId: "iri",
      platform: Platform.Kick,
      username: "IMREALLYIMPORTANT",
      viewerCount: 500,
      now,
    });

    const metrics = getPlatformViewerMetrics("iri");
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

  it.effect("maps an Entity write failure to PersistenceError", () =>
    Effect.gen(function* () {
      vi.spyOn(PlatformViewerMetricsEntity, "upsert").mockImplementationOnce(() => {
        throw new Error("metrics database unavailable");
      });

      const exit = yield* Effect.exit(
        recordPlatformViewerCountEffect({
          streamerId: "iri",
          platform: Platform.Kick,
          username: "iri",
          viewerCount: 10,
          now: new Date(0),
        }),
      );

      expect(Exit.isFailure(exit)).toBe(true);
      if (Exit.isFailure(exit)) {
        expect(String(exit.cause)).toContain("PersistenceError");
        expect(String(exit.cause)).toContain("metrics database unavailable");
      }
    }),
  );
});
