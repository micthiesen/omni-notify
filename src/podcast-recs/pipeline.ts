import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import config from "../utils/config.js";
import {
  type PodcastAccountClient,
  PodcastQueuePosition,
  type PodcastWriteResult,
  resolvePodcastAccount,
} from "./account.js";
import { resolveCandidates } from "./candidates.js";
import { discoverEpisodes } from "./discovery.js";
import { filterEligibleEpisodes, type PodcastFilterContext } from "./filters.js";
import { selectGuestAppearances } from "./guestSelection.js";
import { discoverGuestAppearances } from "./guests.js";
import { decideEpisodeOutcomes } from "./outcomes.js";
import {
  formatRecentRecommendationsDigest,
  getOpenPodcastRecommendations,
  getPodcastExclusions,
  nextVoiceBatch,
  type PodcastQueueResult,
  type PodcastRecommendationData,
  PodcastRecommendationEntity,
  PodcastRecommendationStatus,
} from "./persistence.js";
import {
  type PodcastSelectionPick,
  researchFinalists,
  selectEpisode,
} from "./selection.js";
import { FINALIST_COUNT, type ScoredEpisode, shortlistEpisodes } from "./shortlist.js";
import { resolveSubscriptions } from "./subscriptions.js";
import { buildTasteDigest } from "./taste.js";
import type { EpisodeCandidate } from "./types.js";
import { loadVoices } from "./voices.js";

/** A pending row older than this from a previous run needs reconciliation. */
const STALE_PENDING_MS = 60 * 60 * 1000;
export const MAX_PODCAST_RECOMMENDATIONS_PER_RUN = 5;
const FINALISTS_PER_REQUESTED_PICK = 2;
/** Above this many guest picks, skip the topic/drama fill to avoid flooding. */
const TOPIC_SUPPRESS_THRESHOLD = 3;
/** Small cushion before the oldest open delivery, for timestamp skew. */
const LISTEN_HISTORY_BUFFER_MS = 24 * 60 * 60 * 1000;

export interface PodcastPipelineOptions {
  dryRun?: boolean;
  /** Cap on Tier-2 (topic/drama) picks; Tier-1 guest picks use their own cap. */
  maxRecommendations?: number;
}

/**
 * Two-tier podcast recommendation run:
 *  - Tier 1: episodes where a followed VOICE guests somewhere new (the point of
 *    the feature) — default-include, capped generously for press-tour weeks.
 *  - Tier 2: standout topic/drama episodes — conservative fill, suppressed when
 *    Tier 1 already delivered plenty.
 * Returns a one-line summary.
 */
export async function runPodcastPipeline(
  logger: Logger,
  logFile?: LogFile,
  options: PodcastPipelineOptions = {},
): Promise<string> {
  const topicTargetMax = options.maxRecommendations ?? 2;
  if (
    !Number.isInteger(topicTargetMax) ||
    topicTargetMax < 1 ||
    topicTargetMax > MAX_PODCAST_RECOMMENDATIONS_PER_RUN
  ) {
    throw new RangeError(
      `maxRecommendations must be an integer from 1 to ${MAX_PODCAST_RECOMMENDATIONS_PER_RUN}`,
    );
  }
  const dryRun = options.dryRun ?? false;

  // 1. Local state. Subscriptions follow the three-state rule: a configured
  //    source that fails aborts the run rather than weakening exclusions.
  const account = resolvePodcastAccount(logger);
  const subscriptions = await resolveSubscriptions(account, logger);
  if (subscriptions.status === "unavailable") {
    logger.warn(`Podcast recommendation run skipped: ${subscriptions.reason}`);
    return `skipped: ${subscriptions.reason}`;
  }

  // 2. Outcome sync + stale-pending reconciliation.
  await syncOutcomes(account, logger, logFile);
  reconcileStalePending(logger);

  // 3. Taste inputs: seed profile + subscribed shows + explicit feedback.
  const tasteDigest = buildTasteDigest(subscriptions.value);
  const recentDigest = formatRecentRecommendationsDigest();
  const filterContext = (): PodcastFilterContext => ({
    now: Date.now(),
    subscribedShowIds: subscriptions.value.showIds,
    subscribedShowTitles: subscriptions.value.normalizedTitles,
    exclusions: getPodcastExclusions(Date.now()),
  });

  // 4a. Tier 1 — guest appearances of followed voices (rotated batch/run).
  const voices = nextVoiceBatch(loadVoices(), config.PODCAST_VOICE_ROTATION_MAX);
  const guestPool = await discoverGuestAppearances(voices, account, logger, logFile);
  const guestEligible = logFiltered(
    filterEligibleEpisodes(guestPool, filterContext()),
    "Guest filtered out",
    logFile,
  );

  // 4b. Tier 2 — topic/drama discovery (secondary fill).
  const topicDiscovered = await discoverEpisodes(
    tasteDigest,
    recentDigest,
    logger,
    logFile,
  );
  const topicPool = topicDiscovered.length
    ? await resolveCandidates(topicDiscovered, account, logger, logFile)
    : [];
  const guestIds = new Set(guestEligible.map((c) => c.episodeId));
  const topicEligible = logFiltered(
    filterEligibleEpisodes(topicPool, filterContext()),
    "Topic filtered out",
    logFile,
  ).filter((c) => !guestIds.has(c.episodeId));

  logger.info(`Eligible: ${guestEligible.length} guest, ${topicEligible.length} topic`);
  if (guestEligible.length === 0 && topicEligible.length === 0) {
    return "no eligible candidates after filtering";
  }

  const recommended: EpisodeCandidate[] = [];
  const commitOrCollect = async (
    candidate: EpisodeCandidate,
    pick: PodcastSelectionPick,
    scores: ShortlistScores | undefined,
  ): Promise<void> => {
    if (dryRun) {
      recommended.push(candidate);
      return;
    }
    const committed = await commit(candidate, pick, scores, account, logger);
    if (!committed) throw new Error("Podcast recommendation notification failed");
    recommended.push(candidate);
  };

  // 5. Tier 1: gate (default-include) and commit up to the guest cap.
  if (guestEligible.length > 0) {
    const guestPicks = await selectGuestAppearances(
      guestEligible,
      tasteDigest,
      logger,
      logFile,
      config.PODCAST_MAX_GUEST_PICKS,
    );
    for (const { candidate, pick } of guestPicks) {
      await commitOrCollect(candidate, pick, undefined);
    }
  }
  const guestCount = recommended.length;

  // 6. Tier 2: conservative topic/drama fill, suppressed after a rich guest week.
  const topicTarget = guestCount >= TOPIC_SUPPRESS_THRESHOLD ? 0 : topicTargetMax;
  let stopReason: string | undefined;
  if (topicTarget > 0 && topicEligible.length > 0) {
    const finalists = await shortlistEpisodes(
      topicEligible,
      tasteDigest,
      logger,
      logFile,
      Math.max(FINALIST_COUNT, topicTarget * FINALISTS_PER_REQUESTED_PICK),
    );
    const research = finalists.length
      ? await researchFinalists(finalists, logger, logFile)
      : new Map<string, string>();
    const remaining = new Map<string, ScoredEpisode>(
      finalists.map((finalist) => [finalist.candidate.episodeId, finalist]),
    );
    while (recommended.length - guestCount < topicTarget && remaining.size > 0) {
      const decision = await selectEpisode(
        [...remaining.values()],
        tasteDigest,
        research,
        logger,
        logFile,
      );
      if (!decision) {
        stopReason = "selection model returned no decision";
        break;
      }
      if (decision.decision === "no_add" || !decision.selected) {
        stopReason = `no_add: ${(decision.no_add_reason ?? "no reason given").slice(0, 120)}`;
        break;
      }
      const selected = remaining.get(decision.selected.candidate_id);
      if (!selected) {
        stopReason = "selection returned an unknown candidate id";
        break;
      }
      remaining.delete(selected.candidate.episodeId);
      await commitOrCollect(selected.candidate, decision.selected, {
        tasteMatch: selected.tasteMatch,
        novelty: selected.novelty,
        composite: selected.composite,
        risks: selected.risks,
      });
    }
  }

  return formatBatchSummary(recommended, guestCount, dryRun, stopReason);
}

type ShortlistScores = NonNullable<PodcastRecommendationData["shortlistScores"]>;

/** Filter, logging the dropped candidates, and return the kept list. */
function logFiltered(
  result: ReturnType<typeof filterEligibleEpisodes>,
  section: string,
  logFile?: LogFile,
): EpisodeCandidate[] {
  if (result.dropped.length > 0) {
    logFile?.section(
      section,
      result.dropped
        .map(
          (d) =>
            `- ${d.candidate.showTitle} — ${d.candidate.episodeTitle}: ${d.reason}`,
        )
        .join("\n"),
    );
  }
  return result.kept;
}

async function syncOutcomes(
  account: PodcastAccountClient | undefined,
  logger: Logger,
  logFile?: LogFile,
): Promise<void> {
  if (!account) return;
  const open = getOpenPodcastRecommendations();
  // Outcome labeling is the only consumer of listen history, and it only acts
  // on open recommendations. With none open, the (expensive) history read is
  // pure waste — skip it entirely.
  if (open.length === 0) return;

  const history = await account.fetchListenHistory(listenHistorySince(open));
  if (history.status === "unavailable") {
    logger.warn(`Listen history unavailable (${history.reason}); skipping outcomes`);
    return;
  }
  const changes = decideEpisodeOutcomes(open, history.value, Date.now());
  for (const change of changes) {
    PodcastRecommendationEntity.patch(
      { recommendationId: change.recommendationId },
      { status: change.status, resolvedAt: Date.now() },
    );
    logger.info(`Outcome: ${change.episodeId} → ${change.status} (${change.reason})`);
  }
  if (changes.length > 0) {
    logFile?.section(
      "Outcome Sync",
      changes.map((c) => `- ${c.episodeId} → ${c.status} (${c.reason})`).join("\n"),
    );
  }
}

/**
 * Listen-history look-back cutoff: just before the oldest open delivery, so
 * the window always covers every open recommendation's post-delivery activity.
 *
 * It must NOT be capped shorter than the oldest delivery — a cutoff that
 * post-dates a delivery hides real playback and mislabels a listened episode
 * as ignored. Under gap-free polling the oldest open rec is only ~30-44 days
 * old; a longer window only occurs after a sync gap (e.g. a Castro outage),
 * where fetching the backlog to label it correctly is exactly what we want.
 * The Castro client's own 180-day window (`HISTORY_WINDOW_MS`) is the only
 * hard bound.
 */
export function listenHistorySince(
  open: PodcastRecommendationData[],
  now = Date.now(),
): number {
  if (open.length === 0) return now;
  const oldestDelivery = Math.min(
    ...open.map((rec) => rec.notifiedAt ?? rec.recommendedAt),
  );
  return oldestDelivery - LISTEN_HISTORY_BUFFER_MS;
}

/**
 * Repair rows left pending by a crash during commit. Castro enqueue is
 * idempotent, but notification delivery cannot be verified after the fact, so
 * stale pending rows are marked failed (a 24h retry exclusion, then a retry
 * will observe already_exists if the enqueue landed).
 */
function reconcileStalePending(logger: Logger): void {
  const stale = PodcastRecommendationEntity.getAll().filter(
    (r) =>
      r.status === PodcastRecommendationStatus.Pending &&
      Date.now() - r.recommendedAt > STALE_PENDING_MS,
  );
  for (const rec of stale) {
    logger.warn(
      `Marking stale pending podcast recommendation as failed: ${rec.showTitle} — ${rec.episodeTitle}`,
    );
    PodcastRecommendationEntity.patch(
      { recommendationId: rec.recommendationId },
      { status: PodcastRecommendationStatus.Failed, resolvedAt: Date.now() },
    );
  }
}

async function commit(
  candidate: EpisodeCandidate,
  pick: PodcastSelectionPick,
  shortlistScores: ShortlistScores | undefined,
  account: PodcastAccountClient | undefined,
  logger: Logger,
): Promise<boolean> {
  const recommendationId = crypto.randomUUID();
  PodcastRecommendationEntity.upsert({
    recommendationId,
    episodeId: candidate.episodeId,
    showId: candidate.showId,
    showTitle: candidate.showTitle,
    episodeTitle: candidate.episodeTitle,
    feedUrl: candidate.feedUrl,
    itunesId: candidate.itunesId,
    artworkUrl: candidate.artworkUrl,
    episodeGuid: candidate.episodeGuid,
    mediaUrl: candidate.mediaUrl,
    episodeUrl: candidate.episodeUrl,
    publishedAt: candidate.publishedAt,
    durationMinutes: candidate.durationMinutes,
    status: PodcastRecommendationStatus.Pending,
    whyForUser: pick.why_for_user,
    caveats: pick.caveats,
    confidence: pick.confidence,
    showGenres: candidate.showGenres,
    discoveredVia: candidate.discoveredVia,
    sourceUrl: candidate.sourceUrl,
    matchedVoices: candidate.matchedVoices,
    shortlistScores,
    runDate: new Date().toISOString().slice(0, 10),
    recommendedAt: Date.now(),
  });

  let queueResult: PodcastQueueResult = "not_queued";
  if (account) {
    const enqueueResult = await account.enqueueEpisode({
      feedUrl: candidate.feedUrl,
      itunesId: candidate.itunesId,
      episodeGuid: candidate.episodeGuid,
      mediaUrl: candidate.mediaUrl,
      showTitle: candidate.showTitle,
      episodeTitle: candidate.episodeTitle,
      // Top of the queue: a ~2/week curated pick should be immediately visible
      // and one tap to play, not buried under the existing backlog.
      position: PodcastQueuePosition.Next,
    });
    queueResult = toQueueResult(enqueueResult);
    if (queueResult === "not_queued") {
      logger.info(
        `Castro enqueue ${enqueueResult}; continuing with recommendation deep link`,
      );
    } else {
      logger.info(
        `Castro queue ${queueResult === "queued" ? "added" : "already contained"}: ${candidate.showTitle} - ${candidate.episodeTitle}`,
      );
    }
  }

  try {
    await notify({
      title: pick.notification.title,
      message: appendQueueNote(pick.notification.message, queueResult),
      url: getRecommendationUrl(recommendationId),
      url_title: "View recommendation",
      token: config.PUSHOVER_PODCAST_TOKEN,
    });
  } catch (error) {
    logger.error(
      `Notification failed for ${candidate.episodeTitle}`,
      (error as Error).message,
    );
    return false;
  }

  PodcastRecommendationEntity.patch(
    { recommendationId },
    {
      status: PodcastRecommendationStatus.Notified,
      notifiedAt: Date.now(),
      queueResult,
    },
  );
  logger.info(`Recommended ${candidate.showTitle} — ${candidate.episodeTitle}`);
  return true;
}

export function toQueueResult(writeResult: PodcastWriteResult): PodcastQueueResult {
  if (writeResult === "added") return "queued";
  if (writeResult === "already_exists") return "already_queued";
  return "not_queued";
}

/** Tell the listener the episode is already waiting in Castro, when it is. */
function appendQueueNote(message: string, queueResult: PodcastQueueResult): string {
  if (queueResult === "not_queued") return message;
  return `${message}\n\n🎧 Added to your Castro queue.`;
}

function getRecommendationUrl(recommendationId: string): string {
  const base = config.RECS_PUBLIC_URL.replace(/\/$/, "");
  return `${base}/podcasts?recommendation=${encodeURIComponent(recommendationId)}`;
}

function formatBatchSummary(
  recommended: EpisodeCandidate[],
  guestCount: number,
  dryRun: boolean,
  stopReason?: string,
): string {
  if (recommended.length === 0) {
    return stopReason ?? "no eligible picks";
  }
  const titles = recommended
    .map((c) => `${c.showTitle} — ${c.episodeTitle}`)
    .join(", ");
  const breakdown = `${guestCount} guest, ${recommended.length - guestCount} topic`;
  const prefix = dryRun ? "dry_run: would recommend" : "recommended";
  const stopped = stopReason ? `; stopped: ${stopReason}` : "";
  return `${prefix} ${recommended.length} (${breakdown}): ${titles}${stopped}`;
}
