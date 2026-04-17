import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { formatDistance, formatDistanceToNow } from "date-fns";
import appConfig from "../utils/config.js";
import { ViewerMetricsService } from "./metrics/index.js";
import {
  getStreamerStatus,
  type StreamerStatus,
  type StreamerStatusLive,
  type StreamerStatusOffline,
  upsertStreamerStatus,
} from "./persistence.js";
import { getNotificationUrlFields, platformConfigs } from "./platforms/index.js";
import type { PlatformBinding, Streamer } from "./streamers.js";
import {
  type BindingFetchResult,
  decideTransition,
  type TickDecision,
} from "./transitions.js";

export default class LiveCheckTask extends ScheduledTask {
  public readonly name = "LiveCheckTask";
  public readonly schedule = "*/20 * * * * *";
  public override readonly jitterMs = 3000;
  public override readonly runOnStartup = true;

  private logger: Logger;
  private streamers: Streamer[];
  private consecutiveUnknowns = new Map<string, number>();
  private metricsService: ViewerMetricsService;

  public constructor(streamers: Streamer[], parentLogger: Logger) {
    super();
    this.streamers = streamers;
    this.logger = parentLogger.extend("LiveCheckTask");
    this.metricsService = new ViewerMetricsService(
      (streamerId) => this.getPushoverToken(streamerId),
      parentLogger,
    );
    this.logStreamers();
  }

  private logStreamers(): void {
    for (const s of this.streamers) {
      const bindings = s.bindings.map((b) => `${b.platform}:${b.username}`).join(", ");
      this.logger.info(`Streamer "${s.displayName}" → ${bindings}`);
    }
  }

  public async run(): Promise<void> {
    await Promise.all(this.streamers.map((s) => this.tickStreamer(s)));
  }

  private async tickStreamer(streamer: Streamer): Promise<void> {
    const results = await Promise.all(
      streamer.bindings.map<Promise<BindingFetchResult>>(async (binding) => ({
        binding,
        status: await platformConfigs[binding.platform].fetchLiveStatus({
          username: binding.username,
        }),
      })),
    );

    for (const r of results) this.logBindingStatus(streamer.displayName, r);

    const previous = getStreamerStatus(streamer.id);
    const decision = decideTransition(streamer.id, previous, results);

    switch (decision.kind) {
      case "all-unknown":
        this.handleAllUnknown(streamer, decision.errors);
        return;
      case "partial-unknown-keep":
        this.consecutiveUnknowns.delete(streamer.id);
        return;
      case "went-live":
        this.consecutiveUnknowns.delete(streamer.id);
        await this.handleWentLive(streamer, previous, decision);
        return;
      case "went-offline":
        this.consecutiveUnknowns.delete(streamer.id);
        await this.handleWentOffline(streamer, decision.previousLive, decision.next);
        return;
      case "still-live":
        this.consecutiveUnknowns.delete(streamer.id);
        await this.handleStillLive(streamer, decision);
        return;
    }
  }

  private logBindingStatus(displayName: string, r: BindingFetchResult): void {
    const where = `${displayName} [${r.binding.platform}:${r.binding.username}]`;
    switch (r.status.status) {
      case "live":
        this.logger.debug(`${where} is live: "${r.status.title}"`);
        break;
      case "offline":
        this.logger.debug(`${where} is offline`);
        break;
      case "unknown":
        this.logger.debug(`${where} unknown: ${r.status.error}`);
        break;
    }
  }

  private handleAllUnknown(streamer: Streamer, errors: string[]): void {
    const count = (this.consecutiveUnknowns.get(streamer.id) ?? 0) + 1;
    this.consecutiveUnknowns.set(streamer.id, count);
    const summary = errors.filter(Boolean).join("; ").slice(0, 300);

    if (count >= 10) {
      this.logger.error(
        `${streamer.displayName}: ${count} consecutive all-unknown ticks: ${summary}`,
      );
    } else if (count >= 3) {
      this.logger.warn(
        `${streamer.displayName}: ${count} consecutive all-unknown ticks: ${summary}`,
      );
    }
  }

  private async handleWentLive(
    streamer: Streamer,
    previous: StreamerStatus,
    decision: Extract<TickDecision, { kind: "went-live" }>,
  ): Promise<void> {
    const { next, summedViewerCount } = decision;
    this.logger.info(
      `${streamer.displayName} is now LIVE (primary ${next.primary.platform}:${next.primary.username})`,
    );

    const message = buildLiveMessage(next.primaryTitle, previous);

    await notify({
      title: `${streamer.displayName} is LIVE!`,
      message,
      token: this.getPushoverToken(streamer.id),
      ...getNotificationUrlFields(next.primary.platform, next.primary.username),
    });

    upsertStreamerStatus(next);
    await this.recordViewersIfAny(streamer, next.primary, summedViewerCount);
  }

  private async handleStillLive(
    streamer: Streamer,
    decision: Extract<TickDecision, { kind: "still-live" }>,
  ): Promise<void> {
    const { next, summedViewerCount, titleChanged, primarySwitched } = decision;

    if (primarySwitched) {
      this.logger.info(
        `${streamer.displayName} primary switched to ${next.primary.platform}:${next.primary.username}`,
      );
    }

    if (titleChanged) {
      this.logger.info(`${streamer.displayName} changed title`);
      await notify({
        title: `${streamer.displayName} changed title`,
        message: next.primaryTitle,
        token: this.getPushoverToken(streamer.id),
        ...getNotificationUrlFields(next.primary.platform, next.primary.username),
      });
    }

    upsertStreamerStatus(next);
    await this.recordViewersIfAny(streamer, next.primary, summedViewerCount);
  }

  private async handleWentOffline(
    streamer: Streamer,
    previousLive: StreamerStatusLive,
    next: StreamerStatusOffline,
  ): Promise<void> {
    this.logger.info(`${streamer.displayName} is now offline`);

    await this.metricsService.flushPendingPeaks({
      streamerId: streamer.id,
      displayName: streamer.displayName,
      urlFields: getNotificationUrlFields(
        previousLive.primary.platform,
        previousLive.primary.username,
      ),
    });

    if (appConfig.OFFLINE_NOTIFICATIONS) {
      const duration = formatDistance(new Date(), previousLive.startedAt);
      const baseText = `Streamed for ${duration}`;
      const message =
        previousLive.maxViewerCount > 0
          ? `${baseText} with ${formatCount(previousLive.maxViewerCount)}.`
          : `${baseText}.`;

      await notify({
        title: `${streamer.displayName} is now offline`,
        message,
        token: this.getPushoverToken(streamer.id),
      });
    }

    upsertStreamerStatus(next);
  }

  private async recordViewersIfAny(
    streamer: Streamer,
    primary: PlatformBinding,
    summedViewerCount: number,
  ): Promise<void> {
    if (summedViewerCount <= 0) return;
    await this.metricsService.recordViewerCount({
      streamerId: streamer.id,
      displayName: streamer.displayName,
      viewerCount: summedViewerCount,
      urlFields: getNotificationUrlFields(primary.platform, primary.username),
    });
  }

  private getPushoverToken(streamerId: string): string | undefined {
    const streamer = this.streamers.find((s) => s.id === streamerId);
    return streamer?.pushoverToken ?? appConfig.PUSHOVER_LIVE_TOKEN;
  }
}

function formatCount(count: number): string {
  return `${count.toLocaleString()} viewers`;
}

function buildLiveMessage(primaryTitle: string, previous: StreamerStatus): string {
  if (previous.isLive || !previous.lastEndedAt || !previous.lastStartedAt) {
    return primaryTitle;
  }
  const ago = formatDistanceToNow(previous.lastEndedAt);
  const duration = formatDistance(previous.lastEndedAt, previous.lastStartedAt);
  const suffix = previous.lastMaxViewerCount
    ? `Last live ${ago} ago for ${duration} with ${formatCount(previous.lastMaxViewerCount)}.`
    : `Last live ${ago} ago for ${duration}.`;
  return `${primaryTitle}\n\n${suffix}`;
}
