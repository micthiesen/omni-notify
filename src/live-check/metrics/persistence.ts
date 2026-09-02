import type { Effect as EffectType } from "effect/Effect";
import type { Docstore } from "@micthiesen/mitools/docstore";
import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Option } from "effect";
import type { PersistenceError } from "../../effect/errors.js";
import { PersistenceError as PersistenceFailure } from "../../effect/errors.js";
import { canonicalBindingKey } from "../identityLinks.js";
import type { Platform } from "../platforms/index.js";
import type { PlatformViewerMetricsData, ViewerMetricsData } from "./types.js";
import { pruneBuckets, updateDailyBucket } from "./windows.js";

const PLATFORM_METRICS_RETENTION_DAYS = 100;

export const ViewerMetricsEntity = new Entity<ViewerMetricsData, ["streamerId"]>(
  "streamer-viewer-metrics",
  ["streamerId"],
);

export function getViewerMetricsEffect(
  streamerId: string,
): EffectType<ViewerMetricsData, PersistenceError, Docstore> {
  return ViewerMetricsEntity.get({ streamerId }).pipe(
    Effect.map(
      Option.getOrElse((): ViewerMetricsData => ({
        streamerId,
        dailyBuckets: [],
        allTimeMax: 0,
        allTimeMaxTimestamp: 0,
      })),
    ),
    Effect.mapError(
      (cause) => new PersistenceFailure({ operation: "read viewer metrics", cause }),
    ),
  );
}

export function upsertViewerMetricsEffect(
  metrics: ViewerMetricsData,
): EffectType<void, PersistenceError, Docstore> {
  return ViewerMetricsEntity.upsert(metrics).pipe(
    Effect.mapError(
      (cause) => new PersistenceFailure({ operation: "upsert viewer metrics", cause }),
    ),
  );
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
}): EffectType<void, PersistenceError, Docstore> {
  const canonical = canonicalBindingKey({
    platform: input.platform,
    username: input.username,
  });
  const username = canonical.slice(canonical.indexOf(":") + 1);
  return Effect.gen(function* () {
    const existing = Option.getOrUndefined(
      yield* PlatformViewerMetricsEntity.get({
        streamerId: input.streamerId,
        platform: input.platform,
        username,
      }),
    );
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
    yield* PlatformViewerMetricsEntity.upsert(metrics);
  }).pipe(
    Effect.mapError(
      (cause) =>
        new PersistenceFailure({ operation: "record platform viewer count", cause }),
    ),
  );
}

export function recordPlatformViewerCountEffect(
  input: Parameters<typeof recordPlatformViewerCount>[0],
): EffectType<void, PersistenceError, Docstore> {
  return recordPlatformViewerCount(input);
}

export function getPlatformViewerMetrics(
  streamerId: string,
): EffectType<PlatformViewerMetricsData[], PersistenceError, Docstore> {
  return PlatformViewerMetricsEntity.getAll().pipe(
    Effect.map((rows) => rows.filter((metrics) => metrics.streamerId === streamerId)),
    Effect.mapError(
      (cause) =>
        new PersistenceFailure({ operation: "list platform viewer metrics", cause }),
    ),
  );
}
