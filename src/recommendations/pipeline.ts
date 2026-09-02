import type { LogFile } from "@micthiesen/mitools/logfile";
import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { Clock, Effect } from "effect";
import config from "../utils/config.js";
import { toDateStamp } from "../utils/dates.js";
import { feedbackUrl } from "../utils/feedbackUrl.js";
import {
  assemblePool,
  enrichCandidates,
  fetchCandidateBuckets,
  type WatchSeed,
} from "./candidates.js";
import { filterEligible } from "./filters.js";
import { completedWatches, formatHistoryDigest } from "./history.js";
import { RESOLUTION_CONFIDENCE_THRESHOLD, resolveIdentity } from "./identity.js";
import {
  fetchInProgressEffect as fetchInProgress,
  fetchLibraryIndexEffect as fetchLibraryIndex,
  fetchWatchHistoryEffect as fetchWatchHistory,
} from "./mediaLibrary.js";
import { decideOutcomes } from "./outcomes.js";
import {
  formatFeedbackDigest,
  getExcludedCanonicalIds,
  getOpenRecommendations,
  RecommendationEntity,
  RecommendationStatus,
} from "./persistence.js";
import {
  researchFinalists,
  type SelectionPick,
  selectRecommendation,
} from "./selection.js";
import {
  FINALIST_COUNT,
  type ScoredCandidate,
  shortlistCandidates,
} from "./shortlist.js";
import { formatTasteProfileDigest } from "./taste/index.js";
import { getLatestTasteProfile } from "./taste/persistence.js";
import { fetchTitleGenreIdsEffect as fetchTitleGenreIds } from "./tmdb/client.js";
import type {
  AddToWatchlistResult,
  Candidate,
  CanonicalId,
  MediaItem,
  WatchedItem,
} from "./types.js";
import {
  addToWatchlistEffect as addToWatchlist,
  fetchWatchlistEffect as fetchWatchlist,
} from "./watchlist.js";
import {
  effectMessage,
  persistenceEffect,
  RecommendationCommitError,
  RecommendationInputError,
  RecommendationIntegrationError,
} from "./effect.js";

const RESOLVE_CONCURRENCY = 4;
/** Full (network-fallback) resolution is reserved for the most recent watches. */
const FULL_RESOLUTION_HISTORY_LIMIT = 60;
/** A pending row older than this from a previous run needs reconciliation. */
const STALE_PENDING_MS = 60 * 60 * 1000;
export const MAX_RECOMMENDATIONS_PER_RUN = 10;
const FINALISTS_PER_REQUESTED_PICK = 2;

export interface RecommendationPipelineOptions {
  dryRun?: boolean;
  maxRecommendations?: number;
}

/** Runs the full recommendation pipeline. Returns a one-line summary. */
export const runRecommendationPipelineEffect = Effect.fn("Recommendations.run")(
  function* (
    logger: Logger,
    logFile?: LogFile,
    options: RecommendationPipelineOptions = {},
  ) {
    const maxRecommendations = options.maxRecommendations ?? 1;
    if (
      !Number.isInteger(maxRecommendations) ||
      maxRecommendations < 1 ||
      maxRecommendations > MAX_RECOMMENDATIONS_PER_RUN
    ) {
      return yield* Effect.fail(
        new RecommendationInputError({
          message: `maxRecommendations must be an integer from 1 to ${MAX_RECOMMENDATIONS_PER_RUN}`,
        }),
      );
    }

    // 1. Pull local state. Unavailable history/watchlist aborts the run: never
    //    recommend (or label outcomes) against missing state.
    const [history, inProgress, library, watchlist] = yield* Effect.all(
      [
        fetchWatchHistory(),
        fetchInProgress(),
        fetchLibraryIndex(),
        fetchWatchlist(),
      ] as const,
      { concurrency: "unbounded" },
    );
    if (history.status === "unavailable") {
      yield* logger.warn(`Recommendation run skipped: ${history.reason}`);
      return `skipped: ${history.reason}`;
    }
    if (watchlist.status === "unavailable") {
      yield* logger.warn(`Recommendation run skipped: ${watchlist.reason}`);
      return `skipped: ${watchlist.reason}`;
    }
    if (inProgress.status === "unavailable") {
      yield* logger.warn(`Recommendation run skipped: ${inProgress.reason}`);
      return `skipped: ${inProgress.reason}`;
    }
    if (library.status === "unavailable") {
      yield* logger.warn(`Recommendation run skipped: ${library.reason}`);
      return `skipped: ${library.reason}`;
    }
    const inProgressItems = inProgress.value;
    const libraryItems = library.value;

    // 2. Resolve identities to canonical TMDB ids.
    const watchedItems = history.value;
    const recentWatched = completedWatches(watchedItems).slice(
      0,
      FULL_RESOLUTION_HISTORY_LIMIT,
    );
    const fullResolutionGuids = new Set([
      ...recentWatched.map((w) => w.guid),
      ...watchlist.value.map((w) => w.guid),
      ...inProgressItems.map((w) => w.guid),
    ]);
    const allItems: MediaItem[] = [
      ...watchedItems,
      ...inProgressItems,
      ...watchlist.value,
      ...libraryItems,
    ];
    const canonicalByGuid = yield* resolveManyEffect(
      allItems,
      fullResolutionGuids,
      logger,
    );
    if (logFile)
      yield* logFile.section(
        "Identity Resolution",
        `${canonicalByGuid.size}/${allItems.length} items resolved`,
      );

    // 3. Outcome sync for open recommendations (bookkeeping only — outcome
    //    labels never feed taste inputs).
    const watchedById = new Map<
      string,
      { completion?: number; viewCount: number; lastViewedAt: number }
    >();
    for (const item of watchedItems) {
      const id = canonicalByGuid.get(item.guid);
      const prior = id ? watchedById.get(id) : undefined;
      if (id && (!prior || item.viewedAt > prior.lastViewedAt)) {
        watchedById.set(id, {
          completion: item.completion,
          viewCount: Math.max(prior?.viewCount ?? 0, item.viewCount),
          lastViewedAt: item.viewedAt,
        });
      }
    }
    const inProgressById = new Map<
      string,
      { progress: number; lastViewedAt?: number }
    >();
    for (const item of inProgressItems) {
      const id = canonicalByGuid.get(item.guid);
      if (id)
        inProgressById.set(id, {
          progress: item.progress,
          lastViewedAt: item.lastViewedAt,
        });
    }
    const watchlistIds = new Set<string>();
    let watchlistUnresolved = 0;
    for (const item of watchlist.value) {
      const id = canonicalByGuid.get(item.guid);
      if (id) watchlistIds.add(id);
      else watchlistUnresolved++;
    }
    // Incomplete Arr identity resolution cannot safely reconcile pending writes.
    const watchlistComplete = watchlistUnresolved === 0;
    if (!watchlistComplete) {
      yield* logger.warn(
        `${watchlistUnresolved} watchlist item(s) unresolved; skipping absence-based outcome labels`,
      );
    }
    const outcomeSyncNow = yield* Clock.currentTimeMillis;
    yield* persistenceEffect("sync recommendation outcomes", () =>
      syncOutcomes({
        watchedById,
        inProgressById,
        inProgressAvailable: true,
        logger,
        logFile,
        now: outcomeSyncNow,
      }),
    );
    yield* reconcileStalePendingEffect(watchlistIds, watchlistComplete, logger);

    // 4. Taste inputs from ground-truth history and explicit user feedback.
    const feedbackDigest = yield* persistenceEffect(
      "read recommendation feedback",
      () => formatFeedbackDigest(),
    );
    const tasteProfile = yield* persistenceEffect("read media taste profile", () =>
      getLatestTasteProfile(),
    );
    const tasteDigest = formatTasteProfileDigest(tasteProfile);
    const historyDigest = `${formatHistoryDigest(watchedItems, inProgressItems)}\n\n${feedbackDigest}\n\n${tasteDigest}`;
    const seeds = yield* buildSeedsEffect(recentWatched, canonicalByGuid, logger);

    // 5. Candidate pool.
    const buckets = yield* fetchCandidateBuckets(seeds, logger);
    const pool = assemblePool(buckets);

    // 6. Hard filters (pure code, before any model call). Note: watched
    //    exclusion is best-effort for old history items whose GUIDs carry no
    //    TMDB id — they only resolve cheaply (cache/GUID parse), so an
    //    unresolvable back-catalog watch could theoretically be re-surfaced.
    //    The alias cache warms over successive runs, shrinking that gap.
    const watchedIds = new Set<string>();
    for (const item of watchedItems) {
      const id = canonicalByGuid.get(item.guid);
      if (id) watchedIds.add(id);
    }
    const libraryIds = new Set<string>();
    for (const item of libraryItems) {
      const id = canonicalByGuid.get(item.guid);
      if (id) libraryIds.add(id);
    }
    const excludedRecommendationIds = yield* persistenceEffect(
      "read recommendation exclusions",
      () => getExcludedCanonicalIds(outcomeSyncNow),
    );
    const { kept, dropped } = filterEligible(pool, {
      watchedIds,
      inProgressIds: new Set(inProgressById.keys()),
      watchlistIds,
      excludedRecommendationIds,
    });
    yield* logger.info(`Candidates: ${pool.length} pooled, ${kept.length} eligible`);
    if (dropped.length > 0) {
      if (logFile)
        yield* logFile.section(
          "Filtered Out",
          dropped.map((d) => `- ${d.title}: ${d.reason}`).join("\n"),
        );
    }
    if (kept.length === 0) return "no eligible candidates after filtering";

    // 7. Cheap-model shortlist.
    const candidates = yield* enrichCandidates(kept, libraryIds, logger);
    const finalists = yield* shortlistCandidates(
      candidates,
      historyDigest,
      logger,
      logFile,
      Math.max(FINALIST_COUNT, maxRecommendations * FINALISTS_PER_REQUESTED_PICK),
    );
    if (finalists.length === 0) return "shortlist returned no scorable candidates";

    // 8. Research the shortlist once, then ask the unchanged one-pick selector
    //    repeatedly against a shrinking set. A no_add decision ends the batch.
    const research = yield* researchFinalists(finalists, logger, logFile);
    const remaining = new Map<string, ScoredCandidate>(
      finalists.map((finalist) => [finalist.candidate.canonicalId, finalist]),
    );
    const recommended: Candidate[] = [];
    let stopReason: string | undefined;

    while (recommended.length < maxRecommendations && remaining.size > 0) {
      const decision = yield* selectRecommendation(
        [...remaining.values()],
        historyDigest,
        research,
        logger,
        logFile,
      );
      if (!decision) {
        stopReason = "selection model returned no decision";
        break;
      }
      if (decision.decision === "no_add" || !decision.selected) {
        const reason = decision.no_add_reason ?? "no reason given";
        yield* logger.info(`No further recommendation today: ${reason}`);
        stopReason = `no_add: ${reason.slice(0, 120)}`;
        break;
      }

      const selected = remaining.get(decision.selected.candidate_id);
      if (!selected) {
        yield* logger.warn(
          `Selection returned unknown candidate id: ${decision.selected.candidate_id}`,
        );
        stopReason = "selection returned an unknown candidate id";
        break;
      }
      remaining.delete(selected.candidate.canonicalId);

      if (options.dryRun) {
        recommended.push(selected.candidate);
        continue;
      }

      // 9. Commit: record pending BEFORE the external write so a crash can be
      //    reconciled instead of orphaning a real watchlist addition.
      const commitResult = yield* commitRecommendationEffect(
        selected,
        decision.selected,
        logger,
      );
      if (commitResult === "committed") {
        recommended.push(selected.candidate);
        continue;
      }
      if (commitResult === "failed") {
        return yield* Effect.fail(
          new RecommendationCommitError({
            message: "Recommendation acquisition or notification failed",
          }),
        );
      }

      const backup = decision.backup
        ? remaining.get(decision.backup.candidate_id)
        : undefined;
      if (
        decision.backup &&
        backup &&
        backup.candidate.canonicalId !== selected.candidate.canonicalId
      ) {
        remaining.delete(backup.candidate.canonicalId);
        yield* logger.info(
          `Primary already on watchlist; promoting backup ${backup.candidate.title}`,
        );
        const backupCommitted = yield* commitRecommendationEffect(
          backup,
          decision.backup,
          logger,
          true,
        );
        if (backupCommitted === "committed") {
          recommended.push(backup.candidate);
          continue;
        }
        if (backupCommitted === "failed") {
          return yield* Effect.fail(
            new RecommendationCommitError({
              message: "Recommendation acquisition or notification failed",
            }),
          );
        }
      }
      stopReason = "no_add: selected and backup are already tracked";
      break;
    }

    return formatBatchSummary(
      recommended,
      maxRecommendations,
      options.dryRun ?? false,
      stopReason,
    );
  },
);

function resolveManyEffect(
  items: MediaItem[],
  fullResolutionGuids: Set<string>,
  logger: Logger,
) {
  const resolved = new Map<string, CanonicalId>();
  const seen = new Set<string>();
  const unique = items.filter((item) => {
    if (seen.has(item.guid)) return false;
    seen.add(item.guid);
    return true;
  });
  return Effect.forEach(
    unique,
    (item) =>
      resolveIdentity(item, logger, {
        allowNetwork: fullResolutionGuids.has(item.guid),
      }).pipe(
        Effect.tap((resolution) =>
          Effect.sync(() => {
            if (
              resolution.canonicalId &&
              resolution.confidence >= RESOLUTION_CONFIDENCE_THRESHOLD
            ) {
              resolved.set(item.guid, resolution.canonicalId);
            }
          }),
        ),
      ),
    { concurrency: RESOLVE_CONCURRENCY, discard: true },
  ).pipe(Effect.as(resolved));
}

const syncOutcomes = Effect.fn("Recommendations.syncOutcomes")(function* (args: {
  watchedById: Map<
    string,
    { completion?: number; viewCount: number; lastViewedAt?: number }
  >;
  inProgressById: Map<string, { progress: number; lastViewedAt?: number }>;
  inProgressAvailable: boolean;
  logger: Logger;
  logFile?: LogFile;
  now: number;
}) {
  const open = yield* getOpenRecommendations();
  const now = args.now;
  for (const rec of open) {
    const history = args.watchedById.get(rec.canonicalId);
    const deliveredAt = rec.notifiedAt ?? rec.recommendedAt;
    const watchedAfterDelivery =
      history !== undefined &&
      (history.lastViewedAt === undefined || history.lastViewedAt >= deliveredAt);
    const progress = args.inProgressById.get(rec.canonicalId);
    const progressAfterDelivery =
      progress !== undefined &&
      (progress.lastViewedAt === undefined || progress.lastViewedAt >= deliveredAt);
    if (!rec.startedAt && (watchedAfterDelivery || progressAfterDelivery)) {
      const observedAt = watchedAfterDelivery
        ? history?.lastViewedAt
        : progressAfterDelivery
          ? progress.lastViewedAt
          : undefined;
      yield* RecommendationEntity.patch(
        { recommendationId: rec.recommendationId },
        { startedAt: Math.max(deliveredAt, observedAt ?? now) },
      );
      rec.startedAt = Math.max(deliveredAt, observedAt ?? now);
    }
  }
  const changes = decideOutcomes(open, {
    watched: args.watchedById,
    inProgress: args.inProgressById,
    inProgressAvailable: args.inProgressAvailable,
    now,
  });
  for (const change of changes) {
    yield* RecommendationEntity.patch(
      { recommendationId: change.recommendationId },
      { status: change.status, resolvedAt: now },
    );
    yield* args.logger.info(
      `Outcome: ${change.canonicalId} → ${change.status} (${change.reason})`,
    );
  }
  if (changes.length > 0) {
    if (args.logFile) {
      yield* args.logFile.section(
        "Outcome Sync",
        changes.map((c) => `- ${c.canonicalId} → ${c.status} (${c.reason})`).join("\n"),
      );
    }
  }
});

/**
 * Repair rows left in pending by a crash between the entity write and the
 * notification: if the watchlist add demonstrably landed (or was skipped),
 * send the missed notification now; otherwise mark the row failed.
 */
function reconcileStalePendingEffect(
  watchlistIds: Set<string>,
  watchlistComplete: boolean,
  logger: Logger,
) {
  return Effect.gen(function* () {
    const now = yield* Clock.currentTimeMillis;
    const stale = yield* persistenceEffect("read stale recommendations", () =>
      RecommendationEntity.getAll().pipe(
        Effect.map((rows) =>
          rows.filter(
            (r) =>
              r.status === RecommendationStatus.Pending &&
              now - r.recommendedAt > STALE_PENDING_MS,
          ),
        ),
      ),
    );
    for (const rec of stale) {
      const acquisitionLanded =
        rec.watchlistResult === "available" || watchlistIds.has(rec.canonicalId);
      if (acquisitionLanded && rec.whyForUser) {
        if (rec.notificationState === "reserved" || rec.notificationState === "sent") {
          yield* logger.warn(
            `Reconciling pending recommendation ${rec.canonicalId}: notification attempt already reserved`,
          );
          yield* persistenceEffect(
            "acknowledge reserved recommendation notification",
            () =>
              RecommendationEntity.patch(
                { recommendationId: rec.recommendationId },
                {
                  status: RecommendationStatus.Notified,
                  notificationState:
                    rec.notificationState === "reserved" ? "unknown" : "sent",
                  notifiedAt: rec.notificationReservedAt ?? now,
                },
              ),
          );
          continue;
        }
        const message = rec.whyForUser;
        yield* logger.warn(
          `Reconciling pending recommendation ${rec.canonicalId}: re-notifying`,
        );
        const reservedAt = now;
        yield* persistenceEffect("reserve reconciled recommendation notification", () =>
          RecommendationEntity.patch(
            { recommendationId: rec.recommendationId },
            { notificationState: "reserved", notificationReservedAt: reservedAt },
          ),
        );
        const notificationResult = yield* Effect.result(
          notify({
            title: `🎬 ${rec.title}${rec.year ? ` (${rec.year})` : ""}`,
            message,
            url: feedbackUrl("recommendations", rec.recommendationId),
            url_title: "Rate this pick",
            token: config.PUSHOVER_RECS_TOKEN,
          }).pipe(
            Effect.mapError(
              (cause) =>
                new RecommendationIntegrationError({
                  operation: "notify reconciled recommendation",
                  cause,
                }),
            ),
          ),
        );
        if (notificationResult._tag === "Failure") {
          yield* persistenceEffect(
            "record reconciled recommendation notification failure",
            () =>
              RecommendationEntity.patch(
                { recommendationId: rec.recommendationId },
                { notificationState: "failed" },
              ),
          );
          yield* logger.warn(
            `Reconciled notification failed for ${rec.canonicalId}`,
            effectMessage(notificationResult.failure),
          );
          continue;
        }
        yield* persistenceEffect("mark reconciled recommendation notified", () =>
          RecommendationEntity.patch(
            { recommendationId: rec.recommendationId },
            {
              status: RecommendationStatus.Notified,
              notificationState: "sent",
              notifiedAt: reservedAt,
            },
          ),
        );
      } else if (watchlistComplete) {
        yield* logger.warn(
          `Marking stale pending recommendation ${rec.canonicalId} as failed`,
        );
        yield* persistenceEffect("mark stale recommendation failed", () =>
          RecommendationEntity.patch(
            { recommendationId: rec.recommendationId },
            { status: RecommendationStatus.Failed, resolvedAt: now },
          ),
        );
      } else {
        // The watchlist view is incomplete, so absence proves nothing; leave
        // the row pending and try again next run.
        yield* logger.warn(
          `Leaving stale pending ${rec.canonicalId} unreconciled (watchlist incomplete)`,
        );
      }
    }
  });
}

function buildSeedsEffect(
  recentWatched: WatchedItem[],
  canonicalByGuid: Map<string, CanonicalId>,
  logger: Logger,
) {
  return Effect.gen(function* () {
    const seeds: WatchSeed[] = [];
    for (const item of recentWatched) {
      const canonicalId = canonicalByGuid.get(item.guid);
      if (!canonicalId) continue;
      const tmdbId = Number(canonicalId.split(":")[2]);
      const genreIds = yield* fetchTitleGenreIds(item.mediaType, tmdbId).pipe(
        Effect.catch((error) => {
          return logger
            .warn(`Genre lookup failed for ${canonicalId}`, effectMessage(error))
            .pipe(Effect.as([]));
        }),
      );
      seeds.push({ canonicalId, tmdbId, mediaType: item.mediaType, genreIds });
      if (seeds.length >= 20) break;
    }
    return seeds;
  });
}

function commitRecommendationEffect(
  scored: ScoredCandidate,
  pick: SelectionPick,
  logger: Logger,
  wasBackup = false,
) {
  return Effect.gen(function* () {
    const now = yield* Clock.currentTimeMillis;
    const { candidate } = scored;
    const recommendationId = crypto.randomUUID();
    yield* persistenceEffect("insert pending recommendation", () =>
      RecommendationEntity.upsert({
        recommendationId,
        canonicalId: candidate.canonicalId,
        tmdbId: candidate.tmdbId,
        mediaType: candidate.mediaType,
        title: candidate.title,
        year: candidate.year,
        posterPath: candidate.posterPath,
        status: RecommendationStatus.Pending,
        whyForUser: pick.why_for_user,
        caveats: pick.caveats,
        confidence: pick.confidence,
        source: candidate.source,
        genres: candidate.genres,
        runtimeMinutes: candidate.runtimeMinutes,
        seasonCount: candidate.seasonCount,
        episodeCount: candidate.episodeCount,
        seriesStatus: candidate.seriesStatus,
        originalLanguage: candidate.originalLanguage,
        originCountries: candidate.originCountries,
        creators: candidate.creators,
        cast: candidate.cast,
        keywords: candidate.keywords,
        certification: candidate.certification,
        shortlistScores: {
          tasteMatch: scored.tasteMatch,
          novelty: scored.novelty,
          effortFit: scored.effortFit,
          composite: scored.composite,
          risks: scored.risks,
        },
        runDate: toDateStamp(now),
        recommendedAt: now,
        wasBackup,
      }),
    );

    const addOutcome: {
      result: AddToWatchlistResult | "available";
      titleSlug?: string;
    } = candidate.inLibrary
      ? { result: "available" }
      : yield* addToWatchlist({
          tmdbId: candidate.tmdbId,
          mediaType: candidate.mediaType,
          title: candidate.title,
          year: candidate.year,
          externalIds: { tmdb: candidate.tmdbId },
        });
    const addResult = addOutcome.result;
    const managerSlug = addOutcome.titleSlug;

    if (addResult === "already_exists") {
      yield* logger.warn(`${candidate.title} is already tracked`);
      yield* persistenceEffect("close already tracked recommendation", () =>
        RecommendationEntity.patch(
          { recommendationId },
          {
            status: RecommendationStatus.Failed,
            watchlistResult: "already_exists",
            resolvedAt: now,
            ...(managerSlug ? { managerSlug } : {}),
          },
        ),
      );
      return "already_exists";
    }

    if (addResult !== "added" && addResult !== "available") {
      yield* logger.warn(`Acquisition failed for ${candidate.title} (${addResult})`);
      yield* persistenceEffect("mark recommendation acquisition failed", () =>
        RecommendationEntity.patch(
          { recommendationId },
          {
            status: RecommendationStatus.Failed,
            watchlistResult: "error",
            resolvedAt: now,
          },
        ),
      );
      return "failed";
    }

    const watchlistResult = addResult;
    yield* persistenceEffect("record recommendation acquisition", () =>
      RecommendationEntity.patch(
        { recommendationId },
        { watchlistResult, ...(managerSlug ? { managerSlug } : {}) },
      ),
    );

    const notificationReservedAt = now;
    yield* persistenceEffect("reserve recommendation notification", () =>
      RecommendationEntity.patch(
        { recommendationId },
        { notificationState: "reserved", notificationReservedAt },
      ),
    );
    const notificationResult = yield* Effect.result(
      notify({
        title: pick.notification.title,
        message: pick.notification.message,
        url: feedbackUrl("recommendations", recommendationId),
        url_title: "Rate this pick",
        token: config.PUSHOVER_RECS_TOKEN,
      }).pipe(
        Effect.mapError(
          (cause) =>
            new RecommendationIntegrationError({
              operation: "send recommendation notification",
              cause,
            }),
        ),
      ),
    );
    if (notificationResult._tag === "Failure") {
      yield* logger.error(
        `Notification failed for ${candidate.title}`,
        effectMessage(notificationResult.failure),
      );
      yield* persistenceEffect("record recommendation notification failure", () =>
        RecommendationEntity.patch(
          { recommendationId },
          { notificationState: "failed" },
        ),
      );
      return "failed";
    }

    yield* persistenceEffect("mark recommendation notified", () =>
      RecommendationEntity.patch(
        { recommendationId },
        {
          status: RecommendationStatus.Notified,
          notifiedAt: notificationReservedAt,
          notificationState: "sent",
          watchlistResult,
        },
      ),
    );
    yield* logger.info(
      `Recommended ${candidate.title} (acquisition: ${watchlistResult})`,
    );
    return "committed";
  });
}

function formatTitle(candidate: Candidate): string {
  return candidate.year ? `${candidate.title} (${candidate.year})` : candidate.title;
}

function formatBatchSummary(
  recommended: Candidate[],
  requested: number,
  dryRun: boolean,
  stopReason?: string,
): string {
  if (recommended.length === 0) {
    return stopReason ?? "no_add: no remaining finalists";
  }
  const titles = recommended.map(formatTitle).join(", ");
  if (dryRun) {
    return requested === 1
      ? `dry_run: would recommend ${titles}`
      : `dry_run: would recommend ${recommended.length}/${requested}: ${titles}`;
  }
  if (requested === 1) return `recommended: ${titles}`;
  const stopped = stopReason ? `; stopped: ${stopReason}` : "";
  return `recommended ${recommended.length}/${requested}: ${titles}${stopped}`;
}
