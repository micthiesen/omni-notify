import type { Effect as EffectType } from "effect/Effect";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { formatDistance } from "date-fns";
import { Cause, Clock, Effect, Option } from "effect";
import { IntegrationError, PersistenceError } from "../effect/errors.js";
import { fromPromise, runPromise } from "../effect/interop.js";
import appConfig from "../utils/config.js";
import {
  canonicalBinding,
  fetchDggFeed,
  resolveDggStreams,
  type DggFeed,
} from "./dgg.js";
import {
  forgetProfileIdentityLinkEffect,
  getAllProfileIdentityLinksEffect,
  type ProfileIdentityLink,
} from "./identityLinks.js";
import type { LivestreamIntelligenceObserver } from "./intelligence/service.js";
import { ViewerMetricsService } from "./metrics/index.js";
import { recordPlatformViewerCountEffect } from "./metrics/persistence.js";
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
  getStreamerStatusEffect,
  type StreamerStatus,
  type StreamerStatusLive,
  type StreamerStatusOffline,
  upsertStreamerStatusEffect,
} from "./persistence.js";
import {
  type FetchedStatus,
  getNotificationUrlFields,
  LiveStatus,
  type Platform,
  platformConfigs,
} from "./platforms/index.js";
import { learnProfileIdentityEffect } from "./profileLinks.js";
import { recordCompletedSessionEffect } from "./sessions.js";
import { isStreamerDue, type Streamer } from "./streamers.js";
import { TitleChangeDebouncer } from "./titleDebounce.js";
import {
  type BindingFetchResult,
  decideTransition,
  type TickDecision,
} from "./transitions.js";

const PROFILE_IDENTITY_RETRY_MS = 24 * 60 * 60 * 1000;
const PROFILE_IDENTITY_VERIFICATION_MS = 7 * 24 * 60 * 60 * 1000;

function externalEffect<A>(
  operation: string,
  evaluate: () => A | PromiseLike<A>,
): EffectType<A, unknown> {
  return Effect.suspend(() => {
    try {
      const value = evaluate();
      return value && typeof (value as PromiseLike<A>).then === "function"
        ? fromPromise(operation, () => value as PromiseLike<A>)
        : Effect.succeed(value as A);
    } catch (cause) {
      return Effect.fail(cause);
    }
  });
}

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
    private readonly reconcileIOSControls?: () => EffectType<void, unknown>,
    private readonly dggDiscovery?: {
      topEmbeds: number;
      availablePlatforms: ReadonlySet<Platform>;
      fetchFeed?: () => EffectType<DggFeed, unknown> | Promise<DggFeed>;
      learnIdentity?: (
        input: Parameters<typeof learnProfileIdentityEffect>[0],
      ) =>
        | ReturnType<typeof learnProfileIdentityEffect>
        | Promise<ProfileIdentityLink | undefined>;
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

  public run(): Promise<void> {
    return runPromise(this.runEffect());
  }

  public runEffect(): EffectType<void, unknown> {
    return Effect.gen({ self: this }, function* () {
      // Background streamers skip ticks entirely (not just their notification
      // paths) — a skipped tick means no fetch, no transition, and no
      // unknown-streak change for that streamer this round. The startup run
      // (tick 0) always includes them.
      const tick = this.tickCount++;
      if (this.dggDiscovery && isStreamerDue("background", tick)) {
        yield* this.refreshDggStreamers();
      }
      const due = this.streamers.filter((s) => isStreamerDue(s.tier, tick));
      yield* Effect.forEach(due, (streamer) => this.tickStreamer(streamer), {
        concurrency: 6,
        discard: true,
      });
      if (this.intelligence) yield* this.intelligence.afterTick();
      // Background streamers' unknown streaks (and thus outage detection) only
      // advance on their slower cadence, since they're skipped above otherwise.
      yield* this.reportOutage();
      if (this.reconcileIOSControls) {
        yield* this.reconcileIOSControls().pipe(
          Effect.catch((error) =>
            Effect.sync(() => {
              // Controls are a convenience surface. APNs or control-state failures
              // must never turn a successful live-status tick into a failed task run.
              this.logger.warn(
                `Failed to reconcile iOS controls: ${(error as Error).message}`,
              );
            }),
          ),
        );
      }
    });
  }

  private refreshDggStreamers(): EffectType<void, PersistenceError> {
    if (!this.dggDiscovery) return Effect.void;
    const discovery = this.dggDiscovery;

    return Effect.gen({ self: this }, function* () {
      let resolution: ReturnType<typeof resolveDggStreams>;
      const feed = yield* Effect.suspend(() => {
        const feedValue = (discovery.fetchFeed ?? fetchDggFeed)();
        return Effect.isEffect(feedValue)
          ? (feedValue as EffectType<DggFeed, unknown>)
          : fromPromise("fetch DGG feed", () => feedValue);
      }).pipe(
        Effect.mapError(
          (cause) =>
            new IntegrationError({
              operation: "fetch DGG feed",
              cause,
            }),
        ),
      );
      let identityLinks = yield* getAllProfileIdentityLinksEffect();
      const resolve = () =>
        resolveDggStreams({
          feed,
          limit: this.dggDiscovery?.topEmbeds ?? 0,
          configuredStreamers: this.configuredStreamers,
          availablePlatforms:
            this.dggDiscovery?.availablePlatforms ?? new Set<Platform>(),
          identityAliases: new Map(
            identityLinks.map((link) => [link.sourceBinding, link.targetBinding]),
          ),
        });
      resolution = resolve();
      const configuredBindings = this.configuredStreamers.flatMap(
        (streamer) => streamer.bindings,
      );
      const now = yield* Clock.currentTimeMillis;
      const linksBySource = new Map(
        identityLinks.map((link) => [link.sourceBinding, link]),
      );
      const identityCandidates = [
        ...resolution.discovered.map((entry) => ({ entry, verify: false })),
        ...[...resolution.configuredSources.values()]
          .flat()
          .filter((entry) => {
            const source = entry.streamer.bindings[0];
            if (!source) return false;
            const link = linksBySource.get(
              canonicalBinding(source.platform, source.username),
            );
            return (
              link !== undefined &&
              now - link.verifiedAt >= PROFILE_IDENTITY_VERIFICATION_MS
            );
          })
          .map((entry) => ({ entry, verify: true })),
      ];
      const identityChanged = yield* Effect.forEach(
        identityCandidates,
        ({ entry, verify }) =>
          Effect.gen({ self: this }, function* () {
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
            const identityValue = (
              this.dggDiscovery?.learnIdentity ?? learnProfileIdentityEffect
            )({
              source,
              configuredBindings,
              forceRefresh: verify,
            });
            const identityEffect: EffectType<ProfileIdentityLink | undefined, unknown> =
              Effect.isEffect(identityValue)
                ? (identityValue as EffectType<
                    ProfileIdentityLink | undefined,
                    unknown
                  >)
                : fromPromise("learn profile identity", () => identityValue);
            const link = yield* identityEffect.pipe(
              Effect.catch((error) => {
                this.logger.debug(
                  `Could not resolve profile identity for ${source.platform}:${source.username}: ${String(error)}`,
                );
                return Effect.succeed(undefined);
              }),
            );
            if (verify && link === undefined) {
              yield* forgetProfileIdentityLinkEffect(source);
              this.logger.info(
                `Removed stale profile identity for ${source.platform}:${source.username}`,
              );
              return true;
            }
            return link !== undefined;
          }),
        { concurrency: 4 },
      );
      if (identityChanged.some(Boolean)) {
        identityLinks = yield* getAllProfileIdentityLinksEffect();
        resolution = resolve();
      }

      const selected = resolution.discovered;
      const nextIds = new Set(selected.map(({ streamer }) => streamer.id));
      const removed = this.streamers.filter(
        (streamer) => streamer.discoverySource === "dgg" && !nextIds.has(streamer.id),
      );
      for (const streamer of removed) {
        const previous = yield* getStreamerStatusEffect(streamer.id);
        if (previous.isLive) {
          // Leaving DGG's top set is not proof that the underlying stream ended,
          // so retire its current status without recording a false completed
          // session or confirming a pending viewer record.
          const now = yield* Clock.currentTimeMillis;
          yield* upsertStreamerStatusEffect({
            streamerId: streamer.id,
            isLive: false,
            lastEndedAt: new Date(now),
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
        if (this.intelligence) yield* this.intelligence.observeOffline(streamer.id);
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
        `Destiny.gg discovery selected ${selected.length}/${discovery.topEmbeds}${summary ? `: ${summary}` : ""}`,
      );
    }).pipe(
      Effect.catchCause((cause) => {
        if (Cause.hasInterrupts(cause)) return Effect.interrupt;
        const failure = Cause.findErrorOption(cause);
        if (Option.isSome(failure) && failure.value instanceof PersistenceError) {
          return Effect.fail(failure.value);
        }
        return Effect.sync(() => {
          const message = String(cause);
          this.logger.warn(`Failed to refresh Destiny.gg embeds: ${message}`);
          for (const binding of this.dggStatuses.keys()) {
            this.dggStatuses.set(binding, {
              status: LiveStatus.Unknown,
              error: message,
            });
          }
        });
      }),
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
  private reportOutage(): EffectType<void> {
    return Effect.gen({ self: this }, function* () {
      const alert = this.outageAlerter.evaluate(
        [...this.unknownStreaks.values()],
        this.streamers.length,
        yield* Clock.currentTimeMillis,
      );
      if (!alert) return;

      const line = `${alert.title}\n${alert.message}`;
      if (alert.kind === "degraded") this.logger.warn(line);
      else this.logger.info(line);

      yield* externalEffect("send live-check outage notification", () =>
        notify({ title: alert.title, message: alert.message }),
      ).pipe(
        Effect.catch((error) =>
          Effect.sync(() => {
            // A failed send must not fail the run — the outage state has already
            // advanced, so retrying this exact alert isn't possible anyway.
            this.logger.error(
              "Failed to send live-check outage notification",
              error instanceof Error ? error.message : String(error),
            );
          }),
        ),
      );
    });
  }

  private tickStreamer(streamer: Streamer): EffectType<void, unknown> {
    return Effect.gen({ self: this }, function* () {
      const results = yield* Effect.forEach(
        streamer.bindings,
        (binding) => {
          const discovered = this.dggStatuses.get(
            canonicalBinding(binding.platform, binding.username),
          );
          const statusEffect = discovered
            ? Effect.succeed(discovered)
            : platformConfigs[binding.platform].fetchLiveStatus({
                username: binding.username,
              });
          return statusEffect.pipe(
            Effect.map((status): BindingFetchResult => ({ binding, status })),
          );
        },
        { concurrency: 4 },
      );

      for (const r of results) this.logBindingStatus(streamer.displayName, r);

      const previous = yield* getStreamerStatusEffect(streamer.id);
      const now = yield* Clock.currentTimeMillis;
      const decision = decideTransition(streamer.id, previous, results, new Date(now));

      if (decision.kind === "all-unknown") {
        this.handleAllUnknown(streamer, decision.errors);
        return;
      }
      this.clearUnknownStreak(streamer);
      switch (decision.kind) {
        case "no-change":
          return;
        case "went-live":
          yield* this.handleWentLive(streamer, previous, decision);
          return;
        case "went-offline":
          yield* this.handleWentOffline(streamer, decision.previousLive, decision.next);
          return;
        case "still-live":
          yield* this.handleStillLive(streamer, decision);
          return;
      }
    });
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

  private handleWentLive(
    streamer: Streamer,
    previous: StreamerStatus,
    decision: Extract<TickDecision, { kind: "went-live" }>,
  ): EffectType<void, unknown> {
    return Effect.gen({ self: this }, function* () {
      const { next, summedViewerCount } = decision;
      const now = yield* Clock.currentTimeMillis;
      this.logger.info(
        `${streamer.displayName} is now LIVE (primary ${next.primary.platform}:${next.primary.username})`,
      );

      // The go-live notification below already carries the title, so it counts
      // as the debouncer's baseline — a quick post-live title fix is held for
      // the cooldown rather than notified separately.
      this.titleDebouncer.seed(streamer.id, next.primaryTitle, now);

      // Persist the observed edge before any notification. A delivery failure
      // must not make the next tick rediscover the same edge and send it twice.
      yield* upsertStreamerStatusEffect(next);

      if (this.notificationPermissions(streamer).wentLive) {
        const message = buildLiveMessage(next.primaryTitle, previous, now);

        yield* externalEffect("send went-live notification", () =>
          notify({
            title: `${streamer.displayName} is LIVE!`,
            message,
            token: this.getPushoverToken(streamer.id),
            ...getNotificationUrlFields(
              next.primary.platform,
              next.primary.username,
              next.primary.urlOverride,
            ),
          }),
        );
      }

      yield* this.recordViewersIfAny(streamer, next, summedViewerCount);
      if (this.intelligence) {
        yield* this.intelligence.observeLive({
          streamer,
          status: next,
          wentLive: true,
          titleChanged: false,
        });
      }
    });
  }

  private handleStillLive(
    streamer: Streamer,
    decision: Extract<TickDecision, { kind: "still-live" }>,
  ): EffectType<void, unknown> {
    return Effect.gen({ self: this }, function* () {
      const { next, summedViewerCount, titleChanged, primarySwitched } = decision;
      const now = yield* Clock.currentTimeMillis;

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

      // The observation is authoritative even when a later notification fails.
      yield* upsertStreamerStatusEffect(next);

      // Observed on every still-live tick, not just when the title changed —
      // a title held from an earlier tick needs the chance to fire once its
      // cooldown expires even on a tick with no change of its own.
      if (this.notificationPermissions(streamer).titleChange) {
        const debounced = this.titleDebouncer.observe(streamer.id, {
          currentTitle: next.primaryTitle,
          titleChanged,
          now,
        });
        if (debounced.action === "notify") {
          yield* externalEffect("send title-change notification", () =>
            notify({
              title: `${streamer.displayName} changed title`,
              message: debounced.title,
              token: this.getPushoverToken(streamer.id),
              ...getNotificationUrlFields(
                next.primary.platform,
                next.primary.username,
                next.primary.urlOverride,
              ),
            }),
          );
        }
      }

      yield* this.recordViewersIfAny(streamer, next, summedViewerCount);
      if (this.intelligence) {
        yield* this.intelligence.observeLive({
          streamer,
          status: next,
          wentLive: false,
          titleChanged,
        });
      }
    });
  }

  private handleWentOffline(
    streamer: Streamer,
    previousLive: StreamerStatusLive,
    next: StreamerStatusOffline,
  ): EffectType<void, unknown> {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      this.logger.info(`${streamer.displayName} is now offline`);
      this.titleDebouncer.clear(streamer.id);

      yield* this.metricsService.flushPendingPeaksEffect({
        streamerId: streamer.id,
        displayName: streamer.displayName,
        urlFields: getNotificationUrlFields(
          previousLive.primary.platform,
          previousLive.primary.username,
          previousLive.primary.urlOverride,
        ),
      });

      if (this.notificationPermissions(streamer).wentOffline) {
        const duration = formatDistance(new Date(now), previousLive.startedAt);
        const baseText = `Streamed for ${duration}`;
        const message =
          previousLive.maxViewerCount > 0
            ? `${baseText} with ${formatCount(previousLive.maxViewerCount)}.`
            : `${baseText}.`;

        yield* externalEffect("send went-offline notification", () =>
          notify({
            title: `${streamer.displayName} is now offline`,
            message,
            token: this.getPushoverToken(streamer.id),
          }),
        );
      }

      // Keep the durable live state as the retry marker until the offline alert
      // has been delivered. If Pushover fails, the next confirmed-offline tick
      // retries the alert instead of losing it. The session is closed only after
      // delivery, so retries cannot append duplicate completed sessions.
      yield* recordCompletedSessionEffect(
        previousLive,
        new Date(next.lastEndedAt ?? now),
      );
      yield* upsertStreamerStatusEffect(next);

      if (this.intelligence) yield* this.intelligence.observeOffline(streamer.id);
    });
  }

  private recordViewersIfAny(
    streamer: Streamer,
    status: StreamerStatusLive,
    summedViewerCount: number,
  ): EffectType<void, unknown> {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      if (summedViewerCount > 0) {
        yield* this.metricsService.recordViewerCountEffect({
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
        yield* recordPlatformViewerCountEffect({
          streamerId: streamer.id,
          platform: source.platform,
          username: source.username,
          viewerCount: source.viewerCount,
          now: new Date(now),
        });
      }
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

function buildLiveMessage(
  primaryTitle: string,
  previous: StreamerStatus,
  now: number,
): string {
  if (previous.isLive || !previous.lastEndedAt || !previous.lastStartedAt) {
    return primaryTitle;
  }
  const ago = formatDistance(previous.lastEndedAt, new Date(now));
  const duration = formatDistance(previous.lastEndedAt, previous.lastStartedAt);
  const suffix = previous.lastMaxViewerCount
    ? `Last live ${ago} ago for ${duration} with ${formatCount(previous.lastMaxViewerCount)}.`
    : `Last live ${ago} ago for ${duration}.`;
  return `${primaryTitle}\n\n${suffix}`;
}
