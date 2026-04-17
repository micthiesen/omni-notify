import { Entity } from "@micthiesen/mitools/entities";
import type { ViewerMetricsData } from "./types.js";

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

export function upsertViewerMetrics(metrics: ViewerMetricsData): void {
  ViewerMetricsEntity.upsert(metrics);
}
