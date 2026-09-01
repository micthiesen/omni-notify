import { Entity } from "@micthiesen/mitools/entities";
import type { Effect } from "effect";
import type { PersistenceError } from "../../effect/errors.js";
import { fromSync } from "../../effect/interop.js";
import { canonicalBindingKey } from "../identityLinks.js";
import type { Platform } from "../platforms/index.js";
import type { PlatformViewerMetricsData, ViewerMetricsData } from "./types.js";
import { pruneBuckets, updateDailyBucket } from "./windows.js";

const PLATFORM_METRICS_RETENTION_DAYS = 100;

export const ViewerMetricsEntity = new Entity<ViewerMetricsData, ["streamerId"]>(
  "streamer-viewer-metrics",
  ["streamerId"],
);

export function getViewerMetrics(streamerId: string): ViewerMetricsData {
  return (
    ViewerMetricsEntity.get({ streamerId }) ?? {
      streamerId,
      dailyBuckets: [],
      allTimeMax: 0,
      allTimeMaxTimestamp: 0,
    }
  );
}

export function getViewerMetricsEffect(
  streamerId: string,
): Effect.Effect<ViewerMetricsData, PersistenceError> {
  return fromSync("read viewer metrics", () => getViewerMetrics(streamerId));
}

export function upsertViewerMetrics(metrics: ViewerMetricsData): void {
  ViewerMetricsEntity.upsert(metrics);
}

export function upsertViewerMetricsEffect(
  metrics: ViewerMetricsData,
): Effect.Effect<void, PersistenceError> {
  return fromSync("upsert viewer metrics", () => upsertViewerMetrics(metrics));
}

export const PlatformViewerMetricsEntity = new Entity<
  PlatformViewerMetricsData,
  ["streamerId", "platform", "username"]
>("streamer-platform-viewer-metrics", ["streamerId", "platform", "username"]);

export function recordPlatformViewerCount(input: {
  streamerId: string;
  platform: Platform;
  username: string;
  viewerCount: number;
  now?: Date;
}): void {
  const canonical = canonicalBindingKey({
    platform: input.platform,
    username: input.username,
  });
  const username = canonical.slice(canonical.indexOf(":") + 1);
  const existing = PlatformViewerMetricsEntity.get({
    streamerId: input.streamerId,
    platform: input.platform,
    username,
  });
  const metrics: PlatformViewerMetricsData = existing ?? {
    streamerId: input.streamerId,
    platform: input.platform,
    username,
    dailyBuckets: [],
    allTimeMax: 0,
    allTimeMaxTimestamp: 0,
  };
  metrics.dailyBuckets = updateDailyBucket(
    metrics.dailyBuckets,
    input.viewerCount,
    input.now,
  );
  metrics.dailyBuckets = pruneBuckets(
    metrics.dailyBuckets,
    PLATFORM_METRICS_RETENTION_DAYS,
  );
  if (input.viewerCount > metrics.allTimeMax) {
    metrics.allTimeMax = input.viewerCount;
    metrics.allTimeMaxTimestamp = (input.now ?? new Date()).getTime();
  }
  PlatformViewerMetricsEntity.upsert(metrics);
}

export function recordPlatformViewerCountEffect(
  input: Parameters<typeof recordPlatformViewerCount>[0],
): Effect.Effect<void, PersistenceError> {
  return fromSync("record platform viewer count", () =>
    recordPlatformViewerCount(input),
  );
}

export function getPlatformViewerMetrics(
  streamerId: string,
): PlatformViewerMetricsData[] {
  return PlatformViewerMetricsEntity.getAll().filter(
    (metrics) => metrics.streamerId === streamerId,
  );
}
