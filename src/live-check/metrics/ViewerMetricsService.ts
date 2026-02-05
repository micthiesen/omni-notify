import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { getNotificationUrlFields, type Platform } from "../platforms/index.js";
import { getViewerMetrics, upsertViewerMetrics } from "./persistence.js";
import {
  type ChannelPeakState,
  type ConfirmedPeak,
  MetricWindow,
  type PendingPeak,
  WINDOW_CONFIGS,
} from "./types.js";
import { calculateWindowMax, pruneBuckets, updateDailyBucket } from "./windows.js";

const HYSTERESIS = 0.95; // Peak confirmed when viewers drop below peak × 0.95
const MAX_BUCKET_AGE_DAYS = 100;

export class ViewerMetricsService {
  private logger: Logger;
  private channelStates = new Map<string, ChannelPeakState>();

  constructor(parentLogger: Logger) {
    this.logger = parentLogger.extend("ViewerMetrics");
  }

  /**
   * Record a viewer count observation. Updates daily buckets and tracks peaks.
   * When a peak is confirmed (viewers drop below peak × hysteresis), sends notification.
   */
  async recordViewerCount({
    username,
    displayName,
    platform,
    viewerCount,
  }: {
    username: string;
    displayName: string;
    platform: Platform;
    viewerCount: number;
  }): Promise<void> {
    const channelKey = this.getChannelKey(username, platform);
    const metrics = getViewerMetrics(username, platform);
    const state = this.getOrCreateChannelState(channelKey);

    // Always update today's daily bucket
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
          // Still climbing - update pending peak
          pending.value = viewerCount;
          this.logger.debug(
            `${channelKey}: Updated pending ${config.label} to ${viewerCount}`,
          );
        } else if (viewerCount < pending.value * HYSTERESIS) {
          // Peak confirmed!
          confirmedPeaks.push({
            windowId,
            config,
            peak: pending.value,
            previous: pending.previousMax,
          });
          state.pendingPeaks.delete(windowId);

          // Update all-time max in persistent storage
          if (windowId === MetricWindow.AllTime && pending.value > metrics.allTimeMax) {
            metrics.allTimeMax = pending.value;
            metrics.allTimeMaxTimestamp = Date.now();
          }

          this.logger.debug(
            `${channelKey}: Confirmed ${config.label} peak at ${pending.value}`,
          );
        }
        // else: viewers between (pending * 0.95) and pending - still tracking
      } else if (viewerCount > windowMax) {
        // Start tracking new potential peak
        state.pendingPeaks.set(windowId, {
          value: viewerCount,
          previousMax: windowMax,
        });
        this.logger.debug(
          `${channelKey}: Started tracking ${config.label} peak at ${viewerCount} (prev: ${windowMax})`,
        );
      }
    }

    // Prune old buckets and persist
    metrics.dailyBuckets = pruneBuckets(metrics.dailyBuckets, MAX_BUCKET_AGE_DAYS);
    upsertViewerMetrics(metrics);

    // Send notification for highest priority peak only
    if (confirmedPeaks.length > 0) {
      await this.sendNotification(confirmedPeaks, displayName, platform, username);
    }
  }

  /**
   * Flush any pending peaks when a stream goes offline.
   * This ensures we don't miss peaks that were never confirmed by a drop.
   */
  async flushPendingPeaks(
    username: string,
    displayName: string,
    platform: Platform,
  ): Promise<void> {
    const channelKey = this.getChannelKey(username, platform);
    const state = this.channelStates.get(channelKey);

    if (!state || state.pendingPeaks.size === 0) {
      return;
    }

    const metrics = getViewerMetrics(username, platform);
    const confirmedPeaks: ConfirmedPeak[] = [];

    for (const [windowId, pending] of state.pendingPeaks) {
      const config = WINDOW_CONFIGS[windowId];

      confirmedPeaks.push({
        windowId,
        config,
        peak: pending.value,
        previous: pending.previousMax,
      });

      // Update all-time max if needed
      if (windowId === MetricWindow.AllTime && pending.value > metrics.allTimeMax) {
        metrics.allTimeMax = pending.value;
        metrics.allTimeMaxTimestamp = Date.now();
      }

      this.logger.debug(
        `${channelKey}: Flushed pending ${config.label} peak at ${pending.value}`,
      );
    }

    // Clear channel state
    state.pendingPeaks.clear();

    // Persist any all-time max updates
    upsertViewerMetrics(metrics);

    // Send notification for highest priority peak only
    if (confirmedPeaks.length > 0) {
      await this.sendNotification(confirmedPeaks, displayName, platform, username);
    }
  }

  private getChannelKey(username: string, platform: Platform): string {
    return `${platform}:${username}`;
  }

  private getOrCreateChannelState(channelKey: string): ChannelPeakState {
    let state = this.channelStates.get(channelKey);
    if (!state) {
      state = { pendingPeaks: new Map<MetricWindow, PendingPeak>() };
      this.channelStates.set(channelKey, state);
    }
    return state;
  }

  /**
   * Send notification for the highest priority confirmed peak.
   * Only sends one notification even if multiple windows have new records.
   */
  private async sendNotification(
    confirmedPeaks: ConfirmedPeak[],
    displayName: string,
    platform: Platform,
    username: string,
  ): Promise<void> {
    // Sort by priority descending, pick highest
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
      ...getNotificationUrlFields(platform, username),
    });
  }
}

function formatCount(count: number): string {
  return `${count.toLocaleString()} viewers`;
}
