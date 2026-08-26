import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it } from "vitest";
import { Platform } from "../platforms/index.js";
import {
  getPlatformViewerMetrics,
  PlatformViewerMetricsEntity,
  recordPlatformViewerCount,
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
});
