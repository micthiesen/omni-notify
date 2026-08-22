import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { formatDistance, formatDistanceToNow } from "date-fns";
import appConfig from "../utils/config.js";
import { ViewerMetricsService } from "./metrics/index.js";
import {
  getNotificationPermissions,
  liveNotificationsEnabled,
  type NotificationPermissions,
  type ViewerRecordScope,
} from "./notificationPolicy.js";
import {
  OutageAlerter,
  UNREACHABLE_TICK_THRESHOLD,
  type UnknownStreak,
} from "./outage.js";
import {
  getStreamerStatus,
  type StreamerStatus,
  type StreamerStatusLive,
  type StreamerStatusOffline,
  upsertStreamerStatus,
} from "./persistence.js";
import { getNotificationUrlFields, platformConfigs } from "./platforms/index.js";
import { recordCompletedSession } from "./sessions.js";
import { isStreamerDue, type PlatformBinding, type Streamer } from "./streamers.js";
import { TitleChangeDebouncer } from "./titleDebounce.js";
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
  private streamersById: Map<string, Streamer>;
  private unknownStreaks = new Map<string, UnknownStreak>();
  private outageAlerter = new OutageAlerter();
  private metricsService: ViewerMetricsService;
  private titleDebouncer = new TitleChangeDebouncer();
  private tickCount = 0;

  public constructor(
    streamers: Streamer[],
    parentLogger: Logger,
    private readonly reconcileIOSControls?: () => Promise<void>,
  ) {
    super();
    this.streamers = streamers;
    this.streamersById = new Map(streamers.map((s) => [s.id, s]));
    this.logger = parentLogger.extend("LiveCheckTask");
    this.metricsService = new ViewerMetricsService(
      (streamerId) => this.getPushoverToken(streamerId),
      (streamerId) => this.resolveViewerRecordScope(streamerId),
      parentLogger,
    );
    this.logStreamers();
  }

  private logStreamers(): void {
    for (const s of this.streamers) {
      const bindings = s.bindings.map((b) => `${b.platform}:${b.username}`).join(", ");
      const marker =
        s.tier === "background"
          ? " (background)"
          : liveNotificationsEnabled(s)
            ? ""
            : " (live notifications off)";
      this.logger.info(`Streamer "${s.displayName}" → ${bindings}${marker}`);
    }
  }

  public async run(): Promise<void> {
    // Background streamers skip ticks entirely (not just their notification
    // paths) — a skipped tick means no fetch, no transition, and no
    // unknown-streak change for that streamer this round. The startup run
    // (tick 0) always includes them.
    const tick = this.tickCount++;
    const due = this.streamers.filter((s) => isStreamerDue(s.tier, tick));
    await Promise.all(due.map((s) => this.tickStreamer(s)));
    // Background streamers' unknown streaks (and thus outage detection) only
    // advance on their slower cadence, since they're skipped above otherwise.
    await this.reportOutage();
    try {
      await this.reconcileIOSControls?.();
    } catch (error) {
      // Controls are a convenience surface. APNs or control-state failures
      // must never turn a successful live-status tick into a failed task run.
      this.logger.warn(`Failed to reconcile iOS controls: ${(error as Error).message}`);
    }
  }

  /**
   * One alert for the whole fleet rather than one per streamer per tick: a
   * platform or network blip fails every streamer at once, and the per-streamer
   * detail belongs in the logs, not in a notification storm.
   *
   * Notifications go out directly instead of via logger.error, because the
   * alerter already owns the cadence and the generic alert throttle would
   * otherwise be free to swallow an escalation or a fresh outage. The log line
   * stays at warn/info for the same reason: those levels don't notify, so the
   * alert can't be delivered twice.
   */
  private async reportOutage(): Promise<void> {
    const alert = this.outageAlerter.evaluate(
      [...this.unknownStreaks.values()],
      this.streamers.length,
      Date.now(),
    );
    if (!alert) return;

    const line = `${alert.title}\n${alert.message}`;
    if (alert.kind === "degraded") this.logger.warn(line);
    else this.logger.info(line);

    try {
      await notify({ title: alert.title, message: alert.message });
    } catch (error) {
      // A failed send must not fail the run — the outage state has already
      // advanced, so retrying this exact alert isn't possible anyway.
      this.logger.error(
        "Failed to send live-check outage notification",
        (error as Error).message,
      );
    }
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

    if (decision.kind === "all-unknown") {
      this.handleAllUnknown(streamer, decision.errors);
      return;
    }
    this.clearUnknownStreak(streamer);
    switch (decision.kind) {
      case "no-change":
        return;
      case "went-live":
        await this.handleWentLive(streamer, previous, decision);
        return;
      case "went-offline":
        await this.handleWentOffline(streamer, decision.previousLive, decision.next);
        return;
      case "still-live":
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

  /**
   * Records the streak only. Notifying is the aggregate outage alerter's job
   * (see reportOutage), so this stays below the level that reaches Pushover.
   */
  private handleAllUnknown(streamer: Streamer, errors: string[]): void {
    const ticks = (this.unknownStreaks.get(streamer.id)?.ticks ?? 0) + 1;
    const error = errors.filter(Boolean).join("; ").slice(0, 300);
    this.unknownStreaks.set(streamer.id, {
      displayName: streamer.displayName,
      ticks,
      error,
    });

    const message = `${streamer.displayName}: ${ticks} consecutive all-unknown ticks: ${error}`;
    if (ticks === UNREACHABLE_TICK_THRESHOLD) this.logger.info(message);
    else this.logger.debug(message);
  }

  private clearUnknownStreak(streamer: Streamer): void {
    const streak = this.unknownStreaks.get(streamer.id);
    if (!streak) return;
    this.unknownStreaks.delete(streamer.id);
    if (streak.ticks >= UNREACHABLE_TICK_THRESHOLD) {
      this.logger.info(
        `${streamer.displayName} reachable again after ${streak.ticks} all-unknown ticks`,
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

    // The go-live notification below already carries the title, so it counts
    // as the debouncer's baseline — a quick post-live title fix is held for
    // the cooldown rather than notified separately.
    this.titleDebouncer.seed(streamer.id, next.primaryTitle, Date.now());

    if (this.notificationPermissions(streamer).wentLive) {
      const message = buildLiveMessage(next.primaryTitle, previous);

      await notify({
        title: `${streamer.displayName} is LIVE!`,
        message,
        token: this.getPushoverToken(streamer.id),
        ...getNotificationUrlFields(next.primary.platform, next.primary.username),
      });
    }

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
      // A title held from the old primary must not survive to be notified
      // under the new one — decideTransition never sets titleChanged on a
      // switch, so nothing else would otherwise clear it.
      this.titleDebouncer.clear(streamer.id);
    }

    if (titleChanged) {
      this.logger.info(`${streamer.displayName} changed title`);
    }

    // Observed on every still-live tick, not just when the title changed —
    // a title held from an earlier tick needs the chance to fire once its
    // cooldown expires even on a tick with no change of its own.
    if (this.notificationPermissions(streamer).titleChange) {
      const debounced = this.titleDebouncer.observe(streamer.id, {
        currentTitle: next.primaryTitle,
        titleChanged,
        now: Date.now(),
      });
      if (debounced.action === "notify") {
        await notify({
          title: `${streamer.displayName} changed title`,
          message: debounced.title,
          token: this.getPushoverToken(streamer.id),
          ...getNotificationUrlFields(next.primary.platform, next.primary.username),
        });
      }
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
    this.titleDebouncer.clear(streamer.id);

    recordCompletedSession(previousLive, new Date(next.lastEndedAt ?? Date.now()));

    await this.metricsService.flushPendingPeaks({
      streamerId: streamer.id,
      displayName: streamer.displayName,
      urlFields: getNotificationUrlFields(
        previousLive.primary.platform,
        previousLive.primary.username,
      ),
    });

    if (this.notificationPermissions(streamer).wentOffline) {
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

  private notificationPermissions(streamer: Streamer): NotificationPermissions {
    return getNotificationPermissions(streamer, {
      offlineNotifications: appConfig.OFFLINE_NOTIFICATIONS,
    });
  }

  private resolveViewerRecordScope(streamerId: string): ViewerRecordScope {
    const streamer = this.streamersById.get(streamerId);
    if (!streamer) return "all";
    return this.notificationPermissions(streamer).viewerRecords;
  }

  private getPushoverToken(streamerId: string): string | undefined {
    return (
      this.streamersById.get(streamerId)?.pushoverToken ?? appConfig.PUSHOVER_LIVE_TOKEN
    );
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
