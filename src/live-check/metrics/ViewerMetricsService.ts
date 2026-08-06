import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import type { ViewerRecordScope } from "../notificationPolicy.js";
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
export type RecordScopeResolver = (streamerId: string) => ViewerRecordScope;

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
  private resolveRecordScope: RecordScopeResolver;

  constructor(
    resolveToken: TokenResolver,
    resolveRecordScope: RecordScopeResolver,
    parentLogger: Logger,
  ) {
    this.resolveToken = resolveToken;
    this.resolveRecordScope = resolveRecordScope;
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
    const scope = this.resolveRecordScope(streamerId);

    metrics.dailyBuckets = updateDailyBucket(metrics.dailyBuckets, viewerCount);

    // Notify-eligible confirmations only — every window is still tracked and
    // persisted below regardless of scope; "all-time-only" just narrows which
    // confirmations are allowed to reach a notification.
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
          state.pendingPeaks.delete(windowId);

          if (windowId === MetricWindow.AllTime && pending.value > metrics.allTimeMax) {
            metrics.allTimeMax = pending.value;
            metrics.allTimeMaxTimestamp = Date.now();
          }

          if (scope === "all" || windowId === MetricWindow.AllTime) {
            confirmedPeaks.push({
              windowId,
              config,
              peak: pending.value,
              previous: pending.previousMax,
            });
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
    const scope = this.resolveRecordScope(streamerId);

    const metrics = getViewerMetrics(streamerId);
    const confirmedPeaks: ConfirmedPeak[] = [];

    for (const [windowId, pending] of state.pendingPeaks) {
      const config = WINDOW_CONFIGS[windowId];
      if (windowId === MetricWindow.AllTime && pending.value > metrics.allTimeMax) {
        metrics.allTimeMax = pending.value;
        metrics.allTimeMaxTimestamp = Date.now();
      }
      if (scope === "all" || windowId === MetricWindow.AllTime) {
        confirmedPeaks.push({
          windowId,
          config,
          peak: pending.value,
          previous: pending.previousMax,
        });
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
