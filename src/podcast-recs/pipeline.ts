import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import config from "../utils/config.js";
import { resolvePodcastAccount } from "./account.js";
import { resolveCandidates } from "./candidates.js";
import { discoverEpisodes } from "./discovery.js";
import { filterEligibleEpisodes } from "./filters.js";
import { decideEpisodeOutcomes } from "./outcomes.js";
import {
  formatRecentRecommendationsDigest,
  getOpenPodcastRecommendations,
  getPodcastExclusions,
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

/** A pending row older than this from a previous run needs reconciliation. */
const STALE_PENDING_MS = 60 * 60 * 1000;
export const MAX_PODCAST_RECOMMENDATIONS_PER_RUN = 5;
const FINALISTS_PER_REQUESTED_PICK = 2;

export interface PodcastPipelineOptions {
  dryRun?: boolean;
  maxRecommendations?: number;
}

/** Runs the full podcast recommendation pipeline. Returns a one-line summary. */
export async function runPodcastPipeline(
  logger: Logger,
  logFile?: LogFile,
  options: PodcastPipelineOptions = {},
): Promise<string> {
  const maxRecommendations = options.maxRecommendations ?? 2;
  if (
    !Number.isInteger(maxRecommendations) ||
    maxRecommendations < 1 ||
    maxRecommendations > MAX_PODCAST_RECOMMENDATIONS_PER_RUN
  ) {
    throw new RangeError(
      `maxRecommendations must be an integer from 1 to ${MAX_PODCAST_RECOMMENDATIONS_PER_RUN}`,
    );
  }

  // 1. Local state. Subscriptions follow the three-state rule: a configured
  //    source that fails aborts the run rather than weakening exclusions.
  const account = resolvePodcastAccount(logger);
  const subscriptions = await resolveSubscriptions(account, logger);
  if (subscriptions.status === "unavailable") {
    logger.warn(`Podcast recommendation run skipped: ${subscriptions.reason}`);
    return `skipped: ${subscriptions.reason}`;
  }

  // 2. Outcome sync — only possible with real listen history (Castro bridge).
  //    Without a data source every open rec would drift to "ignored".
  await syncOutcomes(logger, logFile);

  reconcileStalePending(logger);

  // 3. Taste inputs: seed profile + subscribed shows + explicit feedback.
  const tasteDigest = buildTasteDigest(subscriptions.value);
  const recentDigest = formatRecentRecommendationsDigest();

  // 4. Discovery (web search) → verified candidates (iTunes + RSS).
  const discovered = await discoverEpisodes(tasteDigest, recentDigest, logger, logFile);
  if (discovered.length === 0) return "discovery surfaced no episodes";
  const pool = await resolveCandidates(discovered, logger, logFile);

  // 5. Hard filters (pure code, before any model call).
  const { kept, dropped } = filterEligibleEpisodes(pool, {
    now: Date.now(),
    subscribedShowIds: subscriptions.value.showIds,
    subscribedShowTitles: subscriptions.value.normalizedTitles,
    exclusions: getPodcastExclusions(Date.now()),
  });
  logger.info(`Candidates: ${pool.length} resolved, ${kept.length} eligible`);
  if (dropped.length > 0) {
    logFile?.section(
      "Filtered Out",
      dropped
        .map(
          (d) =>
            `- ${d.candidate.showTitle} — ${d.candidate.episodeTitle}: ${d.reason}`,
        )
        .join("\n"),
    );
  }
  if (kept.length === 0) return "no eligible candidates after filtering";

  // 6. Cheap-model shortlist.
  const finalists = await shortlistEpisodes(
    kept,
    tasteDigest,
    logger,
    logFile,
    Math.max(FINALIST_COUNT, maxRecommendations * FINALISTS_PER_REQUESTED_PICK),
  );
  if (finalists.length === 0) return "shortlist returned no scorable candidates";

  // 7. Research once, then repeatedly ask the one-pick selector against a
  //    shrinking set. A no_add decision ends the batch.
  const research = await researchFinalists(finalists, logger, logFile);
  const remaining = new Map<string, ScoredEpisode>(
    finalists.map((finalist) => [finalist.candidate.episodeId, finalist]),
  );
  const recommended: EpisodeCandidate[] = [];
  let stopReason: string | undefined;

  while (recommended.length < maxRecommendations && remaining.size > 0) {
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
      const reason = decision.no_add_reason ?? "no reason given";
      logger.info(`No further podcast recommendation today: ${reason}`);
      stopReason = `no_add: ${reason.slice(0, 120)}`;
      break;
    }

    const selected = remaining.get(decision.selected.candidate_id);
    if (!selected) {
      logger.warn(
        `Selection returned unknown candidate id: ${decision.selected.candidate_id}`,
      );
      stopReason = "selection returned an unknown candidate id";
      break;
    }
    remaining.delete(selected.candidate.episodeId);

    if (options.dryRun) {
      recommended.push(selected.candidate);
      continue;
    }

    // 8. Commit: pending row BEFORE the notification so a crash between the
    //    two is reconcilable (mirrors the media recs commit protocol).
    const committed = await commitRecommendation(selected, decision.selected, logger);
    if (!committed) {
      throw new Error("Podcast recommendation notification failed");
    }
    recommended.push(selected.candidate);
  }

  return formatBatchSummary(
    recommended,
    maxRecommendations,
    options.dryRun ?? false,
    stopReason,
  );
}

async function syncOutcomes(logger: Logger, logFile?: LogFile): Promise<void> {
  const account = resolvePodcastAccount(logger);
  if (!account) return;
  const history = await account.fetchListenHistory();
  if (history.status === "unavailable") {
    logger.warn(`Listen history unavailable (${history.reason}); skipping outcomes`);
    return;
  }
  const changes = decideEpisodeOutcomes(
    getOpenPodcastRecommendations(),
    history.value,
    Date.now(),
  );
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
 * Repair rows left pending by a crash between the entity write and the
 * notification. The notification is the only external effect and its delivery
 * cannot be verified after the fact, so stale pending rows are marked failed
 * (a 24h retry exclusion, then the episode becomes eligible again).
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

async function commitRecommendation(
  scored: ScoredEpisode,
  pick: PodcastSelectionPick,
  logger: Logger,
): Promise<boolean> {
  const { candidate } = scored;
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
    shortlistScores: {
      tasteMatch: scored.tasteMatch,
      novelty: scored.novelty,
      composite: scored.composite,
      risks: scored.risks,
    },
    runDate: new Date().toISOString().slice(0, 10),
    recommendedAt: Date.now(),
  });

  // Future Castro write path: enqueueEpisode() here once the bridge exists,
  // between the pending write and the notification (see docs/castro-sync.md).

  try {
    await notify({
      title: pick.notification.title,
      message: pick.notification.message,
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
    { status: PodcastRecommendationStatus.Notified, notifiedAt: Date.now() },
  );
  logger.info(`Recommended ${candidate.showTitle} — ${candidate.episodeTitle}`);
  return true;
}

function getRecommendationUrl(recommendationId: string): string {
  const base = config.RECS_PUBLIC_URL.replace(/\/$/, "");
  return `${base}/podcasts?recommendation=${encodeURIComponent(recommendationId)}`;
}

function formatBatchSummary(
  recommended: EpisodeCandidate[],
  requested: number,
  dryRun: boolean,
  stopReason?: string,
): string {
  if (recommended.length === 0) {
    return stopReason ?? "no_add: no remaining finalists";
  }
  const titles = recommended
    .map((c) => `${c.showTitle} — ${c.episodeTitle}`)
    .join(", ");
  if (dryRun) {
    return `dry_run: would recommend ${recommended.length}/${requested}: ${titles}`;
  }
  const stopped = stopReason ? `; stopped: ${stopReason}` : "";
  return `recommended ${recommended.length}/${requested}: ${titles}${stopped}`;
}
