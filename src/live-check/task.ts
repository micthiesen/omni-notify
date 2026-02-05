import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { formatDistance, formatDistanceToNow } from "date-fns";
import { ScheduledTask } from "../scheduling/ScheduledTask.js";
import appConfig from "../utils/config.js";
import { type ChannelsConfig, StreamFilterService } from "./filters/index.js";
import { ViewerMetricsService } from "./metrics/index.js";
import {
  type ChannelStatusLive,
  type ChannelStatusOffline,
  getChannelStatus,
  upsertChannelStatus,
} from "./persistence.js";
import {
  type FetchedStatus,
  type FetchedStatusLive,
  getNotificationUrlFields,
  LiveStatus,
  type Platform,
  type PlatformConfig,
  platformConfigs,
} from "./platforms/index.js";

type ChannelInfo = { username: string; displayName: string };

export default class LiveCheckTask extends ScheduledTask {
  public readonly name = "LiveCheck";
  public readonly schedule = "*/20 * * * * *";
  public override readonly jitterMs = 3000;

  private channels: {
    username: string;
    displayName: string;
    config: PlatformConfig;
  }[] = [];

  private logger: Logger;
  private consecutiveUnknowns = new Map<string, number>();
  private metricsService: ViewerMetricsService;
  private filterService: StreamFilterService;

  public constructor(
    channels: [Platform, ChannelInfo[]][],
    channelsConfig: ChannelsConfig,
    parentLogger: Logger,
  ) {
    super();

    this.validateNoDuplicateUsernames(channels);

    for (const [platform, channelList] of channels) {
      const config = platformConfigs[platform];
      for (const { username, displayName } of channelList) {
        this.channels.push({ username, displayName, config });
      }
    }

    this.logger = parentLogger.extend("LiveCheckTask");
    this.metricsService = new ViewerMetricsService(parentLogger);
    this.filterService = new StreamFilterService(channelsConfig, parentLogger);

    this.filterService.logFilterStatus(
      this.channels.map(({ username, displayName, config }) => ({
        username,
        displayName,
        platform: config.platform,
      })),
    );
  }

  private validateNoDuplicateUsernames(channels: [Platform, ChannelInfo[]][]): void {
    const seen = new Map<string, Platform>();

    for (const [platform, entries] of channels) {
      for (const { username } of entries) {
        const existing = seen.get(username);
        if (existing) {
          throw new Error(
            `Duplicate username "${username}" found on ${existing} and ${platform}. ` +
              "Each username must be unique across all platforms due to database key constraints.",
          );
        }
        seen.set(username, platform);
      }
    }
  }

  public async run(): Promise<void> {
    await Promise.all(
      this.channels.map(async ({ username, displayName, config }) => {
        const channelKey = `${config.platform}:${username}`;
        const fetchedStatus = await config.fetchLiveStatus({ username });
        const previousStatus = getChannelStatus(username, config.platform);

        this.logStatus(displayName, fetchedStatus);

        if (fetchedStatus.status === LiveStatus.Unknown) {
          this.handleUnknownStatus(channelKey, fetchedStatus.error);
          return;
        }

        // Clear consecutive unknowns on successful fetch
        this.consecutiveUnknowns.delete(channelKey);

        if (fetchedStatus.status === LiveStatus.Live && !previousStatus.isLive) {
          await this.handleLiveEvent(
            fetchedStatus,
            previousStatus,
            displayName,
            config,
          );
        } else if (fetchedStatus.status === LiveStatus.Live && previousStatus.isLive) {
          if (fetchedStatus.title !== previousStatus.title) {
            await this.handleTitleChangeEvent(
              fetchedStatus,
              previousStatus,
              displayName,
              config,
            );
          }
        } else if (
          fetchedStatus.status === LiveStatus.Offline &&
          previousStatus.isLive
        ) {
          await this.handleOfflineEvent(previousStatus, displayName, config);
        }

        if (fetchedStatus.status === LiveStatus.Live) {
          this.updateMaxViewerCount(username, config.platform, fetchedStatus);
          // Record viewer count for metrics tracking (peak confirmation system)
          if (fetchedStatus.viewerCount !== undefined) {
            await this.metricsService.recordViewerCount({
              username,
              displayName,
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
    { title, category }: FetchedStatusLive,
    { username, lastEndedAt, lastStartedAt, lastViewerCount }: ChannelStatusOffline,
    displayName: string,
    config: PlatformConfig,
  ): Promise<void> {
    this.logger.info(`${displayName} is now live on ${config.displayName}`);

    // Check filter before sending notification
    const filterResult = await this.filterService.shouldNotify({
      username,
      displayName,
      platform: config.platform,
      title,
      category,
    });

    if (!filterResult.shouldNotify) {
      this.logger.info(`Filtered: ${filterResult.reason}`);
      upsertChannelStatus({
        username,
        platform: config.platform,
        isLive: true,
        startedAt: new Date(),
        title,
        notifiedForStream: false,
      });
      return;
    }

    const lastLiveMessage = (() => {
      if (!lastEndedAt) return null;
      const ago = formatDistanceToNow(lastEndedAt);
      const duration = formatDistance(lastEndedAt, lastStartedAt);
      const text = `Last live ${ago} ago for ${duration}`;
      return lastViewerCount
        ? `${text} with ${formatCount(lastViewerCount)}.`
        : `${text}.`;
    })();
    const detailParts = [category ? `${category}.` : null, lastLiveMessage].filter(
      Boolean,
    );
    const details = detailParts.length > 0 ? detailParts.join(" ") : null;
    const filterReason = filterResult.wasFiltered ? filterResult.reason : null;
    const messageParts = [title, details, filterReason].filter(Boolean);
    const message = messageParts.join("\n\n");

    await notify({
      title: `${displayName} is LIVE on ${config.displayName}!`,
      message,
      ...getNotificationUrlFields(config.platform, username),
    });

    upsertChannelStatus({
      username,
      platform: config.platform,
      isLive: true,
      startedAt: new Date(),
      title,
      notifiedForStream: true,
    });
  }

  private async handleOfflineEvent(
    { username, startedAt, maxViewerCount }: ChannelStatusLive,
    displayName: string,
    config: PlatformConfig,
  ): Promise<void> {
    const lastEndedAt = new Date();
    this.logger.info(`${displayName} is now offline on ${config.displayName}`);

    // Flush any pending peak records
    await this.metricsService.flushPendingPeaks(username, displayName, config.platform);

    if (appConfig.OFFLINE_NOTIFICATIONS) {
      const duration = formatDistance(lastEndedAt, startedAt);
      const durationText = `Streamed for ${duration}`;
      const message = maxViewerCount
        ? `${durationText} with ${formatCount(maxViewerCount)}.`
        : `${durationText}.`;

      await notify({ title: `${displayName} is now offline`, message });
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

  private async handleTitleChangeEvent(
    { title, category }: FetchedStatusLive,
    previousStatus: ChannelStatusLive,
    displayName: string,
    config: PlatformConfig,
  ): Promise<void> {
    const { username } = previousStatus;
    this.logger.info(`${displayName} changed title on ${config.displayName}`);

    // If we haven't notified for this stream yet (was filtered), re-check the filter
    // This catches streams that become interesting mid-stream
    // Note: check explicitly for false, not falsy, to handle legacy streams without this field
    if (previousStatus.notifiedForStream === false) {
      const filterResult = await this.filterService.shouldNotify({
        username,
        displayName,
        platform: config.platform,
        title,
        category,
      });

      if (!filterResult.shouldNotify) {
        this.logger.info(`Still filtered after title change: ${filterResult.reason}`);
        upsertChannelStatus({ ...previousStatus, title });
        return;
      }

      // Stream now passes filter - send live notification instead of title change
      this.logger.info(`Stream now passes filter: ${filterResult.reason}`);
      const filterReason = filterResult.wasFiltered ? filterResult.reason : null;
      const message = filterReason ? `${title}\n\n${filterReason}` : title;
      await notify({
        title: `${displayName} is LIVE on ${config.displayName}!`,
        message,
        ...getNotificationUrlFields(config.platform, username),
      });

      upsertChannelStatus({ ...previousStatus, title, notifiedForStream: true });
      return;
    }

    // Normal title change notification
    await notify({
      title: `${displayName} changed title`,
      message: title,
      ...getNotificationUrlFields(config.platform, username),
    });

    upsertChannelStatus({
      ...previousStatus,
      title,
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
