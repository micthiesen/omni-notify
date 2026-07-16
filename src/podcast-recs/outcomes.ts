import type { ListenedEpisode } from "./account.js";
import { normalizeTitle } from "./filters.js";
import {
  type PodcastRecommendationData,
  PodcastRecommendationStatus,
} from "./persistence.js";

/** Fraction of an episode that counts as actually having listened to it. */
export const LISTENED_COMPLETION_THRESHOLD = 0.8;

/** Days after notification with no engagement before a rec counts as ignored. */
export const IGNORE_WINDOW_MS = 30 * 24 * 60 * 60 * 1000;
export const ABANDONED_INACTIVITY_MS = 14 * 24 * 60 * 60 * 1000;

export interface PodcastOutcomeChange {
  recommendationId: string;
  episodeId: string;
  status:
    | PodcastRecommendationStatus.Listened
    | PodcastRecommendationStatus.Abandoned
    | PodcastRecommendationStatus.Ignored;
  reason: string;
}

/**
 * Label outcomes for open (notified) podcast recommendations from the
 * account's listen history. Only ever called when history is actually
 * available — with no data source, every rec would drift to "ignored", so
 * the caller must skip outcome sync entirely while the Castro bridge is
 * unimplemented (see docs/castro-sync.md).
 *
 * Passive labels are bookkeeping for exclusions and the UI. Explicit
 * good-pick/not-for-me feedback is the separate preference signal.
 */
export function decideEpisodeOutcomes(
  open: PodcastRecommendationData[],
  history: ListenedEpisode[],
  now: number,
): PodcastOutcomeChange[] {
  const byGuid = new Map<string, ListenedEpisode>();
  const byTitles = new Map<string, ListenedEpisode>();
  for (const item of history) {
    if (item.episodeGuid) {
      const prior = byGuid.get(item.episodeGuid);
      if (!prior || item.listenedAt > prior.listenedAt) {
        byGuid.set(item.episodeGuid, item);
      }
    }
    const titleKey = titlesKey(item.showTitle, item.episodeTitle);
    const prior = byTitles.get(titleKey);
    if (!prior || item.listenedAt > prior.listenedAt) byTitles.set(titleKey, item);
  }

  const changes: PodcastOutcomeChange[] = [];
  for (const rec of open) {
    if (rec.status !== PodcastRecommendationStatus.Notified) continue;
    const deliveredAt = rec.notifiedAt ?? rec.recommendedAt;
    const listened =
      byGuid.get(rec.episodeGuid) ??
      byTitles.get(titlesKey(rec.showTitle, rec.episodeTitle));
    const engagedAfterDelivery =
      listened !== undefined && listened.listenedAt >= deliveredAt;

    if (
      engagedAfterDelivery &&
      listened &&
      (listened.completion === undefined ||
        listened.completion >= LISTENED_COMPLETION_THRESHOLD)
    ) {
      // No completion data but a playback event counts as listened: podcast
      // clients that report history at all typically only log real plays.
      changes.push({
        recommendationId: rec.recommendationId,
        episodeId: rec.episodeId,
        status: PodcastRecommendationStatus.Listened,
        reason:
          listened.completion === undefined
            ? "playback recorded"
            : `completion=${listened.completion.toFixed(2)}`,
      });
      continue;
    }

    if (
      engagedAfterDelivery &&
      listened?.completion !== undefined &&
      listened.completion < LISTENED_COMPLETION_THRESHOLD &&
      now - listened.listenedAt >= ABANDONED_INACTIVITY_MS
    ) {
      changes.push({
        recommendationId: rec.recommendationId,
        episodeId: rec.episodeId,
        status: PodcastRecommendationStatus.Abandoned,
        reason: `stalled at ${(listened.completion * 100).toFixed(0)}%`,
      });
      continue;
    }

    if (!engagedAfterDelivery && now - deliveredAt > IGNORE_WINDOW_MS) {
      changes.push({
        recommendationId: rec.recommendationId,
        episodeId: rec.episodeId,
        status: PodcastRecommendationStatus.Ignored,
        reason: "no engagement within 30 days",
      });
    }
  }

  return changes;
}

function titlesKey(showTitle: string, episodeTitle: string): string {
  return `${normalizeTitle(showTitle)}::${normalizeTitle(episodeTitle)}`;
}
