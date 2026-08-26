import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { formatDistance, formatDistanceToNow } from "date-fns";
import appConfig from "../utils/config.js";
import { canonicalBinding, fetchDggFeed, resolveDggStreams } from "./dgg.js";
import {
  forgetProfileIdentityLink,
  getProfileIdentityLink,
  ProfileIdentityLinkEntity,
} from "./identityLinks.js";
import type { LivestreamIntelligenceObserver } from "./intelligence/service.js";
import { ViewerMetricsService } from "./metrics/index.js";
import { recordPlatformViewerCount } from "./metrics/persistence.js";
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
import {
  type FetchedStatus,
  getNotificationUrlFields,
  LiveStatus,
  type Platform,
  platformConfigs,
} from "./platforms/index.js";
import { learnProfileIdentity } from "./profileLinks.js";
import { recordCompletedSession } from "./sessions.js";
import { isStreamerDue, type Streamer } from "./streamers.js";
import { TitleChangeDebouncer } from "./titleDebounce.js";
import {
  type BindingFetchResult,
  decideTransition,
  type TickDecision,
} from "./transitions.js";

const PROFILE_IDENTITY_RETRY_MS = 24 * 60 * 60 * 1000;
const PROFILE_IDENTITY_VERIFICATION_MS = 7 * 24 * 60 * 60 * 1000;

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
  private readonly profileIdentityAttempts = new Map<string, number>();
  private readonly configuredStreamers: Streamer[];
  private readonly dggStatuses = new Map<string, FetchedStatus>();

  public constructor(
    streamers: Streamer[],
    parentLogger: Logger,
    private readonly reconcileIOSControls?: () => Promise<void>,
    private readonly dggDiscovery?: {
      topEmbeds: number;
      availablePlatforms: ReadonlySet<Platform>;
      fetchFeed?: typeof fetchDggFeed;
      learnIdentity?: typeof learnProfileIdentity;
    },
    private readonly intelligence?: LivestreamIntelligenceObserver,
  ) {
    super();
    this.streamers = streamers;
    this.configuredStreamers = [...streamers];
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
    if (this.dggDiscovery && isStreamerDue("background", tick)) {
      await this.refreshDggStreamers();
    }
    const due = this.streamers.filter((s) => isStreamerDue(s.tier, tick));
    await Promise.all(due.map((s) => this.tickStreamer(s)));
    this.intelligence?.afterTick();
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

  private async refreshDggStreamers(): Promise<void> {
    if (!this.dggDiscovery) return;

    let resolution: ReturnType<typeof resolveDggStreams>;
    try {
      const feed = await (this.dggDiscovery.fetchFeed ?? fetchDggFeed)();
      const resolve = () =>
        resolveDggStreams({
          feed,
          limit: this.dggDiscovery?.topEmbeds ?? 0,
          configuredStreamers: this.configuredStreamers,
          availablePlatforms:
            this.dggDiscovery?.availablePlatforms ?? new Set<Platform>(),
          identityAliases: new Map(
            ProfileIdentityLinkEntity.getAll().map((link) => [
              link.sourceBinding,
              link.targetBinding,
            ]),
          ),
        });
      resolution = resolve();
      const configuredBindings = this.configuredStreamers.flatMap(
        (streamer) => streamer.bindings,
      );
      const now = Date.now();
      const identityCandidates = [
        ...resolution.discovered.map((entry) => ({ entry, verify: false })),
        ...[...resolution.configuredSources.values()]
          .flat()
          .filter((entry) => {
            const source = entry.streamer.bindings[0];
            if (!source) return false;
            const link = getProfileIdentityLink(source);
            return (
              link !== undefined &&
              now - link.verifiedAt >= PROFILE_IDENTITY_VERIFICATION_MS
            );
          })
          .map((entry) => ({ entry, verify: true })),
      ];
      const identityChanged = await Promise.all(
        identityCandidates.map(async ({ entry, verify }) => {
          const source = entry.streamer.bindings[0];
          if (!source) return false;
          const sourceKey = canonicalBinding(source.platform, source.username);
          const lastAttempt = this.profileIdentityAttempts.get(sourceKey);
          if (
            lastAttempt !== undefined &&
            now - lastAttempt < PROFILE_IDENTITY_RETRY_MS
          ) {
            return false;
          }
          this.profileIdentityAttempts.set(sourceKey, now);
          try {
            const link = await (
              this.dggDiscovery?.learnIdentity ?? learnProfileIdentity
            )({
              source,
              configuredBindings,
              forceRefresh: verify,
            });
            if (verify && link === undefined) {
              forgetProfileIdentityLink(source);
              this.logger.info(
                `Removed stale profile identity for ${source.platform}:${source.username}`,
              );
              return true;
            }
            return link !== undefined;
          } catch (error) {
            this.logger.debug(
              `Could not resolve profile identity for ${source.platform}:${source.username}: ${(error as Error).message}`,
            );
            return false;
          }
        }),
      );
      if (identityChanged.some(Boolean)) resolution = resolve();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.logger.warn(`Failed to refresh Destiny.gg embeds: ${message}`);
      for (const binding of this.dggStatuses.keys()) {
        this.dggStatuses.set(binding, {
          status: LiveStatus.Unknown,
          error: message,
        });
      }
      return;
    }

    const selected = resolution.discovered;
    const nextIds = new Set(selected.map(({ streamer }) => streamer.id));
    const removed = this.streamers.filter(
      (streamer) => streamer.discoverySource === "dgg" && !nextIds.has(streamer.id),
    );
    for (const streamer of removed) {
      const previous = getStreamerStatus(streamer.id);
      if (previous.isLive) {
        // Leaving DGG's top set is not proof that the underlying stream ended,
        // so retire its current status without recording a false completed
        // session or confirming a pending viewer record.
        upsertStreamerStatus({
          streamerId: streamer.id,
          isLive: false,
          lastEndedAt: new Date(),
          lastStartedAt: previous.startedAt,
          lastMaxViewerCount: previous.maxViewerCount,
        });
      }
      this.unknownStreaks.delete(streamer.id);
      this.titleDebouncer.clear(streamer.id);
      this.metricsService.discardPendingPeaks(streamer.id);
      for (const binding of streamer.bindings) {
        this.dggStatuses.delete(canonicalBinding(binding.platform, binding.username));
      }
      this.intelligence?.observeOffline(streamer.id);
    }

    this.dggStatuses.clear();
    for (const entry of selected) {
      const binding = entry.streamer.bindings[0];
      if (binding) {
        this.dggStatuses.set(
          canonicalBinding(binding.platform, binding.username),
          entry.status,
        );
      }
    }
    for (const entries of resolution.configuredSources.values()) {
      for (const entry of entries) {
        const binding = entry.streamer.bindings[0];
        if (binding) {
          this.dggStatuses.set(
            canonicalBinding(binding.platform, binding.username),
            entry.status,
          );
        }
      }
    }
    const enrichedConfigured = this.configuredStreamers.map((streamer) => {
      const dgg = resolution.configuredPresence.get(streamer.id);
      const linkedBindings = (resolution.configuredSources.get(streamer.id) ?? [])
        .flatMap((entry) => entry.streamer.bindings)
        .filter(
          (binding) =>
            !streamer.bindings.some(
              (existing) =>
                canonicalBinding(existing.platform, existing.username) ===
                canonicalBinding(binding.platform, binding.username),
            ),
        );
      return dgg || linkedBindings.length > 0
        ? { ...streamer, dgg, bindings: [...streamer.bindings, ...linkedBindings] }
        : streamer;
    });
    this.streamers.splice(
      0,
      this.streamers.length,
      ...enrichedConfigured,
      ...selected.map(({ streamer }) => streamer),
    );
    this.streamersById = new Map(
      this.streamers.map((streamer) => [streamer.id, streamer]),
    );

    const summary = selected
      .map(
        ({ streamer }) =>
          `${streamer.bindings[0]?.platform}:${streamer.bindings[0]?.username}${
            streamer.dgg?.hosted ? " (hosted)" : ""
          }`,
      )
      .join(", ");
    this.logger.debug(
      `Destiny.gg discovery selected ${selected.length}/${this.dggDiscovery.topEmbeds}${summary ? `: ${summary}` : ""}`,
    );
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
        status:
          this.dggStatuses.get(canonicalBinding(binding.platform, binding.username)) ??
          (await platformConfigs[binding.platform].fetchLiveStatus({
            username: binding.username,
          })),
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
        ...getNotificationUrlFields(
          next.primary.platform,
          next.primary.username,
          next.primary.urlOverride,
        ),
      });
    }

    upsertStreamerStatus(next);
    await this.recordViewersIfAny(streamer, next, summedViewerCount);
    this.intelligence?.observeLive({
      streamer,
      status: next,
      wentLive: true,
      titleChanged: false,
    });
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
          ...getNotificationUrlFields(
            next.primary.platform,
            next.primary.username,
            next.primary.urlOverride,
          ),
        });
      }
    }

    upsertStreamerStatus(next);
    await this.recordViewersIfAny(streamer, next, summedViewerCount);
    this.intelligence?.observeLive({
      streamer,
      status: next,
      wentLive: false,
      titleChanged,
    });
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
        previousLive.primary.urlOverride,
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
    this.intelligence?.observeOffline(streamer.id);
  }

  private async recordViewersIfAny(
    streamer: Streamer,
    status: StreamerStatusLive,
    summedViewerCount: number,
  ): Promise<void> {
    if (summedViewerCount > 0) {
      await this.metricsService.recordViewerCount({
        streamerId: streamer.id,
        displayName: streamer.displayName,
        viewerCount: summedViewerCount,
        urlFields: getNotificationUrlFields(
          status.primary.platform,
          status.primary.username,
          status.primary.urlOverride,
        ),
      });
    }
    for (const source of status.sources ?? []) {
      if (!source.viewerCount || source.viewerCount <= 0) continue;
      recordPlatformViewerCount({
        streamerId: streamer.id,
        platform: source.platform,
        username: source.username,
        viewerCount: source.viewerCount,
      });
    }
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
