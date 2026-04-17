export enum MetricWindow {
  SevenDays = "7d",
  ThirtyDays = "30d",
  NinetyDays = "90d",
  AllTime = "all-time",
}

export type WindowConfig = {
  days: number | null;
  label: string;
  priority: number;
};

export const WINDOW_CONFIGS: Record<MetricWindow, WindowConfig> = {
  [MetricWindow.SevenDays]: { days: 7, label: "7-day high", priority: 1 },
  [MetricWindow.ThirtyDays]: { days: 30, label: "30-day high", priority: 2 },
  [MetricWindow.NinetyDays]: { days: 90, label: "90-day high", priority: 3 },
  [MetricWindow.AllTime]: { days: null, label: "all-time record", priority: 4 },
};

export type DailyBucket = {
  date: string;
  maxViewers: number;
  timestamp: number;
};

export type ViewerMetricsData = {
  streamerId: string;
  dailyBuckets: DailyBucket[];
  allTimeMax: number;
  allTimeMaxTimestamp: number;
};

export type PendingPeak = {
  value: number;
  previousMax: number;
};

export type StreamerPeakState = {
  pendingPeaks: Map<MetricWindow, PendingPeak>;
};

export type ConfirmedPeak = {
  windowId: MetricWindow;
  config: WindowConfig;
  peak: number;
  previous: number;
};
