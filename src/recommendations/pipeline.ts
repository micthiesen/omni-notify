import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import PQueue from "p-queue";
import config from "../utils/config.js";
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
  fetchInProgress,
  fetchLibraryIndex,
  fetchWatchHistory,
} from "./mediaLibrary.js";
import { decideOutcomes } from "./outcomes.js";
import {
  formatFeedbackDigest,
  getExcludedCanonicalIds,
  getOpenRecommendations,
  RecommendationEntity,
  RecommendationStatus,
} from "./persistence.js";
import { type SelectionPick, selectRecommendation } from "./selection.js";
import type { ScoredCandidate } from "./shortlist.js";
import { shortlistCandidates } from "./shortlist.js";
import { formatTasteProfileDigest } from "./taste/index.js";
import { fetchTitleGenreIds } from "./tmdb/client.js";
import type { Candidate, CanonicalId, MediaItem, WatchedItem } from "./types.js";
import { addToWatchlist, fetchWatchlist } from "./watchlist.js";

const RESOLVE_CONCURRENCY = 4;
/** Full (network-fallback) resolution is reserved for the most recent watches. */
const FULL_RESOLUTION_HISTORY_LIMIT = 60;
/** A pending row older than this from a previous run needs reconciliation. */
const STALE_PENDING_MS = 60 * 60 * 1000;

/** Runs the full recommendation pipeline. Returns a one-line summary. */
export async function runRecommendationPipeline(
  logger: Logger,
  logFile?: LogFile,
  options: { dryRun?: boolean } = {},
): Promise<string> {
  // 1. Pull local state. Unavailable history/watchlist aborts the run: never
  //    recommend (or label outcomes) against missing state.
  const [history, inProgress, library, watchlist] = await Promise.all([
    fetchWatchHistory(),
    fetchInProgress(),
    fetchLibraryIndex(),
    fetchWatchlist(),
  ]);
  if (history.status === "unavailable") {
    logger.warn(`Recommendation run skipped: ${history.reason}`);
    return `skipped: ${history.reason}`;
  }
  if (watchlist.status === "unavailable") {
    logger.warn(`Recommendation run skipped: ${watchlist.reason}`);
    return `skipped: ${watchlist.reason}`;
  }
  if (inProgress.status === "unavailable") {
    logger.warn(`Recommendation run skipped: ${inProgress.reason}`);
    return `skipped: ${inProgress.reason}`;
  }
  if (library.status === "unavailable") {
    logger.warn(`Recommendation run skipped: ${library.reason}`);
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
  const canonicalByGuid = await resolveMany(allItems, fullResolutionGuids, logger);
  logFile?.section(
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
  const inProgressById = new Map<string, { progress: number; lastViewedAt?: number }>();
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
    logger.warn(
      `${watchlistUnresolved} watchlist item(s) unresolved; skipping absence-based outcome labels`,
    );
  }
  syncOutcomes({
    watchedById,
    inProgressById,
    inProgressAvailable: true,
    logger,
    logFile,
  });
  await reconcileStalePending(watchlistIds, watchlistComplete, logger);

  // 4. Taste inputs from ground-truth history and explicit user feedback.
  const historyDigest = `${formatHistoryDigest(watchedItems, inProgressItems)}\n\n${formatFeedbackDigest()}\n\n${formatTasteProfileDigest()}`;
  const seeds = await buildSeeds(recentWatched, canonicalByGuid, logger);

  // 5. Candidate pool.
  const buckets = await fetchCandidateBuckets(seeds, logger);
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
  const { kept, dropped } = filterEligible(pool, {
    watchedIds,
    inProgressIds: new Set(inProgressById.keys()),
    watchlistIds,
    excludedRecommendationIds: getExcludedCanonicalIds(Date.now()),
  });
  logger.info(`Candidates: ${pool.length} pooled, ${kept.length} eligible`);
  if (dropped.length > 0) {
    logFile?.section(
      "Filtered Out",
      dropped.map((d) => `- ${d.title}: ${d.reason}`).join("\n"),
    );
  }
  if (kept.length === 0) return "no eligible candidates after filtering";

  // 7. Cheap-model shortlist.
  const candidates = await enrichCandidates(kept, libraryIds, logger);
  const finalists = await shortlistCandidates(
    candidates,
    historyDigest,
    logger,
    logFile,
  );
  if (finalists.length === 0) return "shortlist returned no scorable candidates";

  // 8. Strong-model research + selection.
  const decision = await selectRecommendation(
    finalists,
    historyDigest,
    logger,
    logFile,
  );
  if (!decision) return "selection model returned no decision";
  if (decision.decision === "no_add" || !decision.selected) {
    const reason = decision.no_add_reason ?? "no reason given";
    logger.info(`No recommendation today: ${reason}`);
    return `no_add: ${reason.slice(0, 120)}`;
  }

  // 9. Commit: record pending BEFORE the external write so a crash can be
  //    reconciled instead of orphaning a real watchlist addition.
  const byId = new Map<string, ScoredCandidate>(
    finalists.map((f) => [f.candidate.canonicalId, f]),
  );
  const selected = byId.get(decision.selected.candidate_id);
  if (!selected) {
    logger.warn(
      `Selection returned unknown candidate id: ${decision.selected.candidate_id}`,
    );
    return "selection returned an unknown candidate id";
  }

  if (options.dryRun) {
    return `dry_run: would recommend ${formatTitle(selected.candidate)}`;
  }

  const commitResult = await commitRecommendation(selected, decision.selected, logger);
  if (commitResult === "committed") {
    return `recommended: ${formatTitle(selected.candidate)}`;
  }

  const backup = decision.backup ? byId.get(decision.backup.candidate_id) : undefined;
  if (
    commitResult === "already_exists" &&
    decision.backup &&
    backup &&
    backup.candidate.canonicalId !== selected.candidate.canonicalId
  ) {
    logger.info(
      `Primary already on watchlist; promoting backup ${backup.candidate.title}`,
    );
    const backupCommitted = await commitRecommendation(
      backup,
      decision.backup,
      logger,
      true,
    );
    if (backupCommitted === "committed")
      return `recommended (backup): ${formatTitle(backup.candidate)}`;
  }
  if (commitResult === "already_exists") {
    return "no_add: selected and backup are already tracked";
  }
  throw new Error("Recommendation acquisition or notification failed");
}

async function resolveMany(
  items: MediaItem[],
  fullResolutionGuids: Set<string>,
  logger: Logger,
): Promise<Map<string, CanonicalId>> {
  const queue = new PQueue({ concurrency: RESOLVE_CONCURRENCY });
  const resolved = new Map<string, CanonicalId>();
  const seen = new Set<string>();

  await Promise.all(
    items.map((item) => {
      if (seen.has(item.guid)) return Promise.resolve();
      seen.add(item.guid);
      return queue.add(async () => {
        const resolution = await resolveIdentity(item, logger, {
          allowNetwork: fullResolutionGuids.has(item.guid),
        });
        if (
          resolution.canonicalId &&
          resolution.confidence >= RESOLUTION_CONFIDENCE_THRESHOLD
        ) {
          resolved.set(item.guid, resolution.canonicalId);
        }
      });
    }),
  );
  return resolved;
}

function syncOutcomes(args: {
  watchedById: Map<
    string,
    { completion?: number; viewCount: number; lastViewedAt?: number }
  >;
  inProgressById: Map<string, { progress: number; lastViewedAt?: number }>;
  inProgressAvailable: boolean;
  logger: Logger;
  logFile?: LogFile;
}): void {
  const open = getOpenRecommendations();
  const now = Date.now();
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
      RecommendationEntity.patch(
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
    RecommendationEntity.patch(
      { recommendationId: change.recommendationId },
      { status: change.status, resolvedAt: now },
    );
    args.logger.info(
      `Outcome: ${change.canonicalId} → ${change.status} (${change.reason})`,
    );
  }
  if (changes.length > 0) {
    args.logFile?.section(
      "Outcome Sync",
      changes.map((c) => `- ${c.canonicalId} → ${c.status} (${c.reason})`).join("\n"),
    );
  }
}

/**
 * Repair rows left in pending by a crash between the entity write and the
 * notification: if the watchlist add demonstrably landed (or was skipped),
 * send the missed notification now; otherwise mark the row failed.
 */
async function reconcileStalePending(
  watchlistIds: Set<string>,
  watchlistComplete: boolean,
  logger: Logger,
): Promise<void> {
  const stale = RecommendationEntity.getAll().filter(
    (r) =>
      r.status === RecommendationStatus.Pending &&
      Date.now() - r.recommendedAt > STALE_PENDING_MS,
  );
  for (const rec of stale) {
    const acquisitionLanded =
      rec.watchlistResult === "available" || watchlistIds.has(rec.canonicalId);
    if (acquisitionLanded && rec.whyForUser) {
      logger.warn(
        `Reconciling pending recommendation ${rec.canonicalId}: re-notifying`,
      );
      await notify({
        title: `🎬 ${rec.title}${rec.year ? ` (${rec.year})` : ""}`,
        message: rec.whyForUser,
        url: getRecommendationUrl(rec.recommendationId),
        url_title: "View recommendation",
        token: config.PUSHOVER_RECS_TOKEN,
      });
      RecommendationEntity.patch(
        { recommendationId: rec.recommendationId },
        { status: RecommendationStatus.Notified, notifiedAt: Date.now() },
      );
    } else if (watchlistComplete) {
      logger.warn(`Marking stale pending recommendation ${rec.canonicalId} as failed`);
      RecommendationEntity.patch(
        { recommendationId: rec.recommendationId },
        { status: RecommendationStatus.Failed, resolvedAt: Date.now() },
      );
    } else {
      // The watchlist view is incomplete, so absence proves nothing; leave
      // the row pending and try again next run.
      logger.warn(
        `Leaving stale pending ${rec.canonicalId} unreconciled (watchlist incomplete)`,
      );
    }
  }
}

async function buildSeeds(
  recentWatched: WatchedItem[],
  canonicalByGuid: Map<string, CanonicalId>,
  logger: Logger,
): Promise<WatchSeed[]> {
  const seeds: WatchSeed[] = [];
  for (const item of recentWatched) {
    const canonicalId = canonicalByGuid.get(item.guid);
    if (!canonicalId) continue;
    const tmdbId = Number(canonicalId.split(":")[2]);
    const genreIds = await fetchTitleGenreIds(item.mediaType, tmdbId).catch((error) => {
      logger.warn(`Genre lookup failed for ${canonicalId}`, (error as Error).message);
      return [] as number[];
    });
    seeds.push({ canonicalId, tmdbId, mediaType: item.mediaType, genreIds });
    if (seeds.length >= 20) break;
  }
  return seeds;
}

async function commitRecommendation(
  scored: ScoredCandidate,
  pick: SelectionPick,
  logger: Logger,
  wasBackup = false,
): Promise<"committed" | "already_exists" | "failed"> {
  const { candidate } = scored;
  const recommendationId = crypto.randomUUID();
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
    runDate: new Date().toISOString().slice(0, 10),
    recommendedAt: Date.now(),
    wasBackup,
  });

  const addResult = candidate.inLibrary
    ? "available"
    : await addToWatchlist({
        tmdbId: candidate.tmdbId,
        mediaType: candidate.mediaType,
        title: candidate.title,
        year: candidate.year,
        externalIds: { tmdb: candidate.tmdbId },
      });

  if (addResult === "already_exists") {
    logger.warn(`${candidate.title} is already tracked`);
    RecommendationEntity.patch(
      { recommendationId },
      { status: RecommendationStatus.Failed, watchlistResult: "already_exists" },
    );
    return "already_exists";
  }

  if (addResult !== "added" && addResult !== "available") {
    logger.warn(`Acquisition failed for ${candidate.title} (${addResult})`);
    RecommendationEntity.patch(
      { recommendationId },
      {
        status: RecommendationStatus.Failed,
        watchlistResult: "error",
        resolvedAt: Date.now(),
      },
    );
    return "failed";
  }

  const watchlistResult = addResult;
  RecommendationEntity.patch({ recommendationId }, { watchlistResult });

  try {
    await notify({
      title: pick.notification.title,
      message: pick.notification.message,
      url: getRecommendationUrl(recommendationId),
      url_title: "View recommendation",
      token: config.PUSHOVER_RECS_TOKEN,
    });
  } catch (error) {
    logger.error(
      `Notification failed for ${candidate.title}`,
      (error as Error).message,
    );
    return "failed";
  }

  RecommendationEntity.patch(
    { recommendationId },
    {
      status: RecommendationStatus.Notified,
      notifiedAt: Date.now(),
      watchlistResult,
    },
  );
  logger.info(`Recommended ${candidate.title} (acquisition: ${watchlistResult})`);
  return "committed";
}

function getRecommendationUrl(recommendationId: string): string {
  const base = config.RECS_PUBLIC_URL.replace(/\/$/, "");
  return `${base}/recommendations?recommendation=${encodeURIComponent(recommendationId)}`;
}

function formatTitle(candidate: Candidate): string {
  return candidate.year ? `${candidate.title} (${candidate.year})` : candidate.title;
}
