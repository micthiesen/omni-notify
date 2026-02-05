import { Entity } from "@micthiesen/mitools/entities";
import type { Platform } from "../platforms/index.js";
import type { ViewerMetricsData } from "./types.js";

export const ViewerMetricsEntity = new Entity<
  ViewerMetricsData,
  ["platform", "username"]
>("viewer-metrics", ["platform", "username"]);

export function getViewerMetrics(
  username: string,
  platform: Platform,
): ViewerMetricsData {
  const metrics = ViewerMetricsEntity.get({ platform, username });
  return (
    metrics ?? {
      username,
      platform,
      dailyBuckets: [],
      allTimeMax: 0,
      allTimeMaxTimestamp: 0,
    }
  );
}

export function upsertViewerMetrics(metrics: ViewerMetricsData): void {
  ViewerMetricsEntity.upsert(metrics);
}
