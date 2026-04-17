import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { getViewerMetrics, upsertViewerMetrics } from "./persistence.js";
import {
  type ConfirmedPeak,
  MetricWindow,
  type PendingPeak,
  type StreamerPeakState,
  WINDOW_CONFIGS,
} from "./types.js";
import { calculateWindowMax, pruneBuckets, updateDailyBucket } from "./windows.js";

const HYSTERESIS = 0.95;
const MAX_BUCKET_AGE_DAYS = 100;

export type TokenResolver = (streamerId: string) => string | undefined;

export type NotificationUrlFields = { url: string; url_title: string };

export type ViewerObservation = {
  streamerId: string;
  displayName: string;
  viewerCount: number;
  urlFields: NotificationUrlFields;
};

export class ViewerMetricsService {
  private logger: Logger;
  private streamerStates = new Map<string, StreamerPeakState>();
  private resolveToken: TokenResolver;

  constructor(resolveToken: TokenResolver, parentLogger: Logger) {
    this.resolveToken = resolveToken;
    this.logger = parentLogger.extend("ViewerMetrics");
  }

  async recordViewerCount({
    streamerId,
    displayName,
    viewerCount,
    urlFields,
  }: ViewerObservation): Promise<void> {
    const metrics = getViewerMetrics(streamerId);
    const state = this.getOrCreateStreamerState(streamerId);

    metrics.dailyBuckets = updateDailyBucket(metrics.dailyBuckets, viewerCount);

    const confirmedPeaks: ConfirmedPeak[] = [];

    for (const [windowId, config] of Object.entries(WINDOW_CONFIGS) as [
      MetricWindow,
      (typeof WINDOW_CONFIGS)[MetricWindow],
    ][]) {
      const windowMax = calculateWindowMax(metrics, config);
      const pending = state.pendingPeaks.get(windowId);

      if (pending) {
        if (viewerCount > pending.value) {
          pending.value = viewerCount;
          this.logger.debug(
            `${streamerId}: Updated pending ${config.label} to ${viewerCount}`,
          );
        } else if (viewerCount < pending.value * HYSTERESIS) {
          confirmedPeaks.push({
            windowId,
            config,
            peak: pending.value,
            previous: pending.previousMax,
          });
          state.pendingPeaks.delete(windowId);

          if (windowId === MetricWindow.AllTime && pending.value > metrics.allTimeMax) {
            metrics.allTimeMax = pending.value;
            metrics.allTimeMaxTimestamp = Date.now();
          }

          this.logger.debug(
            `${streamerId}: Confirmed ${config.label} peak at ${pending.value}`,
          );
        }
      } else if (viewerCount > windowMax) {
        state.pendingPeaks.set(windowId, {
          value: viewerCount,
          previousMax: windowMax,
        });
        this.logger.debug(
          `${streamerId}: Started tracking ${config.label} peak at ${viewerCount} (prev: ${windowMax})`,
        );
      }
    }

    metrics.dailyBuckets = pruneBuckets(metrics.dailyBuckets, MAX_BUCKET_AGE_DAYS);
    upsertViewerMetrics(metrics);

    if (confirmedPeaks.length > 0) {
      await this.sendNotification(confirmedPeaks, streamerId, displayName, urlFields);
    }
  }

  async flushPendingPeaks({
    streamerId,
    displayName,
    urlFields,
  }: Omit<ViewerObservation, "viewerCount">): Promise<void> {
    const state = this.streamerStates.get(streamerId);
    if (!state || state.pendingPeaks.size === 0) return;

    const metrics = getViewerMetrics(streamerId);
    const confirmedPeaks: ConfirmedPeak[] = [];

    for (const [windowId, pending] of state.pendingPeaks) {
      const config = WINDOW_CONFIGS[windowId];
      confirmedPeaks.push({
        windowId,
        config,
        peak: pending.value,
        previous: pending.previousMax,
      });
      if (windowId === MetricWindow.AllTime && pending.value > metrics.allTimeMax) {
        metrics.allTimeMax = pending.value;
        metrics.allTimeMaxTimestamp = Date.now();
      }
      this.logger.debug(
        `${streamerId}: Flushed pending ${config.label} peak at ${pending.value}`,
      );
    }

    state.pendingPeaks.clear();
    upsertViewerMetrics(metrics);

    if (confirmedPeaks.length > 0) {
      await this.sendNotification(confirmedPeaks, streamerId, displayName, urlFields);
    }
  }

  private getOrCreateStreamerState(streamerId: string): StreamerPeakState {
    let state = this.streamerStates.get(streamerId);
    if (!state) {
      state = { pendingPeaks: new Map<MetricWindow, PendingPeak>() };
      this.streamerStates.set(streamerId, state);
    }
    return state;
  }

  private async sendNotification(
    confirmedPeaks: ConfirmedPeak[],
    streamerId: string,
    displayName: string,
    urlFields: NotificationUrlFields,
  ): Promise<void> {
    const sorted = [...confirmedPeaks].sort(
      (a, b) => b.config.priority - a.config.priority,
    );
    const highest = sorted[0];

    const title = `New ${highest.config.label} for ${displayName}!`;
    const previousPart =
      highest.previous > 0 ? ` (previous: ${highest.previous.toLocaleString()})` : "";
    const message = `Peaked at ${formatCount(highest.peak)}${previousPart}.`;

    this.logger.info(
      `${displayName}: ${highest.config.label} at ${highest.peak} viewers`,
    );
    await notify({
      title,
      message,
      token: this.resolveToken(streamerId),
      ...urlFields,
    });
  }
}

function formatCount(count: number): string {
  return `${count.toLocaleString()} viewers`;
}
