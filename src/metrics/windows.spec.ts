import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Platform } from "../platforms/index.js";
import {
  MetricWindow,
  type ViewerMetricsData,
  WINDOW_CONFIGS,
  type WindowConfig,
} from "./types.js";
import { calculateWindowMax, pruneBuckets, updateDailyBucket } from "./windows.js";

function getWindowConfig(id: MetricWindow): WindowConfig {
  const config = WINDOW_CONFIGS.find((w) => w.id === id);
  if (!config) throw new Error(`Window config not found: ${id}`);
  return config;
}

describe("updateDailyBucket", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-06-15T12:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("should create a new bucket for today when none exists", () => {
    const buckets = updateDailyBucket([], 1000);
    expect(buckets).toHaveLength(1);
    expect(buckets[0].date).toBe("2024-06-15");
    expect(buckets[0].maxViewers).toBe(1000);
  });

  it("should update today's bucket when new count is higher", () => {
    const existing = [{ date: "2024-06-15", maxViewers: 500, timestamp: 100 }];
    const buckets = updateDailyBucket(existing, 1000);
    expect(buckets).toHaveLength(1);
    expect(buckets[0].maxViewers).toBe(1000);
  });

  it("should not update today's bucket when new count is lower", () => {
    const existing = [{ date: "2024-06-15", maxViewers: 1000, timestamp: 100 }];
    const buckets = updateDailyBucket(existing, 500);
    expect(buckets).toHaveLength(1);
    expect(buckets[0].maxViewers).toBe(1000);
    expect(buckets[0].timestamp).toBe(100); // timestamp unchanged
  });

  it("should preserve existing buckets from other days", () => {
    const existing = [
      { date: "2024-06-14", maxViewers: 800, timestamp: 100 },
      { date: "2024-06-13", maxViewers: 600, timestamp: 50 },
    ];
    const buckets = updateDailyBucket(existing, 1000);
    expect(buckets).toHaveLength(3);
    expect(buckets.find((b) => b.date === "2024-06-15")?.maxViewers).toBe(1000);
    expect(buckets.find((b) => b.date === "2024-06-14")?.maxViewers).toBe(800);
  });

  it("should not mutate the original array", () => {
    const existing = [{ date: "2024-06-15", maxViewers: 500, timestamp: 100 }];
    const result = updateDailyBucket(existing, 1000);
    expect(existing[0].maxViewers).toBe(500); // original unchanged
    expect(result[0].maxViewers).toBe(1000); // new array has update
    expect(result).not.toBe(existing); // different array reference
  });
});

describe("pruneBuckets", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-06-15T12:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("should remove buckets older than maxDays", () => {
    const buckets = [
      { date: "2024-06-15", maxViewers: 1000, timestamp: 100 },
      { date: "2024-06-10", maxViewers: 800, timestamp: 50 },
      { date: "2024-06-01", maxViewers: 600, timestamp: 30 },
      { date: "2024-05-01", maxViewers: 400, timestamp: 10 },
    ];
    const pruned = pruneBuckets(buckets, 30);
    expect(pruned).toHaveLength(3);
    expect(pruned.map((b) => b.date)).toEqual([
      "2024-06-15",
      "2024-06-10",
      "2024-06-01",
    ]);
  });

  it("should keep all buckets if none are too old", () => {
    const buckets = [
      { date: "2024-06-15", maxViewers: 1000, timestamp: 100 },
      { date: "2024-06-14", maxViewers: 800, timestamp: 50 },
    ];
    const pruned = pruneBuckets(buckets, 100);
    expect(pruned).toHaveLength(2);
  });
});

describe("calculateWindowMax", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2024-06-15T12:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  const baseMetrics: ViewerMetricsData = {
    username: "test",
    platform: Platform.Twitch,
    dailyBuckets: [
      { date: "2024-06-15", maxViewers: 1000, timestamp: 100 }, // today
      { date: "2024-06-10", maxViewers: 1500, timestamp: 80 }, // 5 days ago
      { date: "2024-06-01", maxViewers: 2000, timestamp: 60 }, // 14 days ago
      { date: "2024-05-01", maxViewers: 3000, timestamp: 40 }, // 45 days ago
      { date: "2024-04-01", maxViewers: 5000, timestamp: 20 }, // 75 days ago
    ],
    allTimeMax: 10000,
    allTimeMaxTimestamp: 10,
  };

  it("should return all-time max for all-time window", () => {
    const window = getWindowConfig(MetricWindow.AllTime);
    expect(calculateWindowMax(baseMetrics, window)).toBe(10000);
  });

  it("should calculate 7-day max correctly", () => {
    const window = getWindowConfig(MetricWindow.SevenDays);
    // Only includes buckets from 2024-06-15 and 2024-06-10
    expect(calculateWindowMax(baseMetrics, window)).toBe(1500);
  });

  it("should calculate 30-day max correctly", () => {
    const window = getWindowConfig(MetricWindow.ThirtyDays);
    // Includes 2024-06-15, 2024-06-10, 2024-06-01
    expect(calculateWindowMax(baseMetrics, window)).toBe(2000);
  });

  it("should calculate 90-day max correctly", () => {
    const window = getWindowConfig(MetricWindow.NinetyDays);
    // Includes all buckets
    expect(calculateWindowMax(baseMetrics, window)).toBe(5000);
  });

  it("should return 0 for empty buckets in non-all-time windows", () => {
    const emptyMetrics: ViewerMetricsData = {
      ...baseMetrics,
      dailyBuckets: [],
    };
    const window = getWindowConfig(MetricWindow.SevenDays);
    expect(calculateWindowMax(emptyMetrics, window)).toBe(0);
  });
});
