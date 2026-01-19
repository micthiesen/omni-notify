import type { Platform } from "../platforms/index.js";

export enum MetricWindow {
  SevenDays = "7d",
  ThirtyDays = "30d",
  NinetyDays = "90d",
  AllTime = "all-time",
}

export type WindowConfig = {
  id: MetricWindow;
  days: number | null; // null for all-time
  label: string;
  priority: number; // higher = more important
};

export const WINDOW_CONFIGS: WindowConfig[] = [
  { id: MetricWindow.SevenDays, days: 7, label: "7-day high", priority: 1 },
  { id: MetricWindow.ThirtyDays, days: 30, label: "30-day high", priority: 2 },
  { id: MetricWindow.NinetyDays, days: 90, label: "90-day high", priority: 3 },
  { id: MetricWindow.AllTime, days: null, label: "all-time record", priority: 4 },
];

export type DailyBucket = {
  date: string; // YYYY-MM-DD
  maxViewers: number;
  timestamp: number; // when the max was recorded
};

export type ViewerMetricsData = {
  username: string;
  platform: Platform;
  dailyBuckets: DailyBucket[];
  allTimeMax: number;
  allTimeMaxTimestamp: number;
};

export type PendingPeak = {
  value: number;
  previousMax: number;
};

export type ChannelPeakState = {
  pendingPeaks: Map<MetricWindow, PendingPeak>;
};

export type ConfirmedPeak = {
  window: WindowConfig;
  peak: number;
  previous: number;
};
