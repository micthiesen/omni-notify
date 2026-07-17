import { toDateStamp } from "../../utils/dates.js";
import type { DailyBucket, ViewerMetricsData, WindowConfig } from "./types.js";

/**
 * Update daily buckets with a new viewer count observation.
 * If today's bucket exists and the new count is higher, update it.
 * Otherwise, create a new bucket for today.
 * Always returns a new array (does not mutate input).
 */
export function updateDailyBucket(
  buckets: DailyBucket[],
  viewerCount: number,
): DailyBucket[] {
  const today = toDateStamp();
  const now = Date.now();

  const existingIndex = buckets.findIndex((b) => b.date === today);
  if (existingIndex >= 0) {
    const existing = buckets[existingIndex];
    if (viewerCount > existing.maxViewers) {
      // Return new array with updated bucket
      return buckets.map((b, i) =>
        i === existingIndex
          ? { date: today, maxViewers: viewerCount, timestamp: now }
          : b,
      );
    }
    // No change needed, return copy
    return [...buckets];
  }

  // No bucket for today, create one
  return [...buckets, { date: today, maxViewers: viewerCount, timestamp: now }];
}

/**
 * Prune buckets older than the specified number of days
 */
export function pruneBuckets(buckets: DailyBucket[], maxDays: number): DailyBucket[] {
  const cutoffDate = new Date();
  cutoffDate.setDate(cutoffDate.getDate() - maxDays);
  const cutoffString = toDateStamp(cutoffDate.getTime());

  return buckets.filter((b) => b.date >= cutoffString);
}

/**
 * Calculate the maximum viewer count within a rolling window.
 * For all-time windows (days = null), returns the stored all-time max.
 */
export function calculateWindowMax(
  metrics: ViewerMetricsData,
  window: WindowConfig,
): number {
  if (window.days === null) {
    return metrics.allTimeMax;
  }

  const cutoffDate = new Date();
  cutoffDate.setDate(cutoffDate.getDate() - window.days);
  const cutoffString = toDateStamp(cutoffDate.getTime());

  let max = 0;
  for (const bucket of metrics.dailyBuckets) {
    if (bucket.date >= cutoffString && bucket.maxViewers > max) {
      max = bucket.maxViewers;
    }
  }
  return max;
}
