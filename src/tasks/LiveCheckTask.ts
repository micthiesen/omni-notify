import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { formatDistance, formatDistanceToNow } from "date-fns";
import { ViewerMetricsService } from "../metrics/index.js";
import {
  type FetchedStatus,
  type FetchedStatusLive,
  LiveStatus,
  type Platform,
  type PlatformConfig,
  platformConfigs,
} from "../platforms/index.js";
import appConfig from "../utils/config.js";
import {
  type ChannelStatusLive,
  type ChannelStatusOffline,
  getChannelStatus,
  upsertChannelStatus,
} from "./persistence/status.js";
import { Task } from "./types.js";

export default class LiveCheckTask extends Task {
  public name = "Live Check";
  private channels: { username: string; config: PlatformConfig }[] = [];

  private logger: Logger;
  private consecutiveUnknowns = new Map<string, number>();
  private metricsService: ViewerMetricsService;

  public constructor(channels: [Platform, string[]][], parentLogger: Logger) {
    super();
    for (const [platform, usernames] of channels) {
      const config = platformConfigs[platform];
      for (const username of usernames) {
        this.channels.push({ username, config });
      }
    }

    this.logger = parentLogger.extend("LiveCheckTask");
    this.metricsService = new ViewerMetricsService(parentLogger);
  }

  public async run(): Promise<void> {
    await Promise.all(
      this.channels.map(async ({ username, config }) => {
        const channelKey = `${config.platform}:${username}`;
        const fetchedStatus = await config.fetchLiveStatus({ username });
        const previousStatus = getChannelStatus(username, config.platform);

        this.logStatus(username, fetchedStatus);

        if (fetchedStatus.status === LiveStatus.Unknown) {
          this.handleUnknownStatus(channelKey, fetchedStatus.error);
          return;
        }

        // Clear consecutive unknowns on successful fetch
        this.consecutiveUnknowns.delete(channelKey);

        if (fetchedStatus.status === LiveStatus.Live && !previousStatus.isLive) {
          await this.handleLiveEvent(fetchedStatus, previousStatus, config);
        } else if (
          fetchedStatus.status === LiveStatus.Offline &&
          previousStatus.isLive
        ) {
          await this.handleOfflineEvent(previousStatus, config);
        }

        if (fetchedStatus.status === LiveStatus.Live) {
          this.updateMaxViewerCount(username, config.platform, fetchedStatus);
          // Record viewer count for metrics tracking (peak confirmation system)
          if (fetchedStatus.viewerCount !== undefined) {
            await this.metricsService.recordViewerCount({
              username,
              platform: config.platform,
              viewerCount: fetchedStatus.viewerCount,
            });
          }
        }
      }),
    );
  }

  private logStatus(username: string, status: FetchedStatus): void {
    switch (status.status) {
      case LiveStatus.Live:
        this.logger.debug(`${username} is live: "${status.title}"`);
        break;
      case LiveStatus.Offline:
        this.logger.debug(`${username} is offline`);
        break;
      case LiveStatus.Unknown:
        this.logger.debug(`${username} status unknown: ${status.error}`);
        break;
    }
  }

  private handleUnknownStatus(channelKey: string, error: string): void {
    const count = (this.consecutiveUnknowns.get(channelKey) ?? 0) + 1;
    this.consecutiveUnknowns.set(channelKey, count);

    // Escalate logging after repeated failures
    if (count >= 10) {
      this.logger.error(
        `${channelKey}: ${count} consecutive unknown statuses: ${error}`,
      );
    } else if (count >= 3) {
      this.logger.warn(
        `${channelKey}: ${count} consecutive unknown statuses: ${error}`,
      );
    }
  }

  private async handleLiveEvent(
    { title }: FetchedStatusLive,
    { username, lastEndedAt, lastStartedAt, lastViewerCount }: ChannelStatusOffline,
    config: PlatformConfig,
  ): Promise<void> {
    this.logger.info(`${username} is now live on ${config.displayName}`);

    const lastLiveMessage = (() => {
      if (!lastEndedAt) return null;
      const ago = formatDistanceToNow(lastEndedAt);
      const duration = formatDistance(lastEndedAt, lastStartedAt);
      const text = `Last live ${ago} ago for ${duration}`;
      return lastViewerCount ? `${text} with ${formatCount(lastViewerCount)}` : text;
    })();
    const message = lastLiveMessage ? `${title}\n\n${lastLiveMessage}` : title;

    await notify({
      title: `${username} is LIVE on ${config.displayName}!`,
      message,
      url: config.getLiveUrl(username),
      url_title: `Watch on ${config.displayName}`,
    });

    upsertChannelStatus({
      username,
      platform: config.platform,
      isLive: true,
      startedAt: new Date(),
      title,
    });
  }

  private async handleOfflineEvent(
    { username, startedAt, maxViewerCount }: ChannelStatusLive,
    config: PlatformConfig,
  ): Promise<void> {
    const lastEndedAt = new Date();
    this.logger.info(`${username} is now offline on ${config.displayName}`);

    // Flush any pending peak records
    await this.metricsService.flushPendingPeaks(username, config.platform);

    if (appConfig.OFFLINE_NOTIFICATIONS) {
      const duration = formatDistance(lastEndedAt, startedAt);
      const durationText = `Streamed for ${duration}`;
      const message = maxViewerCount
        ? `${durationText} with ${formatCount(maxViewerCount)}`
        : durationText;

      await notify({ title: `${username} is now offline`, message });
    }

    upsertChannelStatus({
      username,
      platform: config.platform,
      isLive: false,
      lastEndedAt,
      lastStartedAt: startedAt,
      lastViewerCount: maxViewerCount,
    });
  }

  private updateMaxViewerCount(
    username: string,
    platform: Platform,
    fetchedStatus: FetchedStatusLive,
  ): void {
    if (fetchedStatus.viewerCount === undefined) return;

    const currentStatus = getChannelStatus(username, platform);
    if (!currentStatus.isLive) return;

    if (
      currentStatus.maxViewerCount === undefined ||
      fetchedStatus.viewerCount > currentStatus.maxViewerCount
    ) {
      currentStatus.maxViewerCount = fetchedStatus.viewerCount;
      upsertChannelStatus(currentStatus);
      this.logger.debug(
        `Updated max viewer count for ${username} to ${fetchedStatus.viewerCount}`,
      );
    }
  }
}

function formatCount(count: number): string {
  return `${count.toLocaleString()} viewers`;
}
