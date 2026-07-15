import { type RecommendationData, RecommendationStatus } from "./persistence.js";

/** Fraction of runtime that counts as actually having watched a title. */
export const WATCHED_COMPLETION_THRESHOLD = 0.8;

/** Days after notification with no engagement before a rec counts as ignored. */
export const IGNORE_WINDOW_MS = 30 * 24 * 60 * 60 * 1000;
export const ABANDONED_INACTIVITY_MS = 14 * 24 * 60 * 60 * 1000;

export interface OutcomeInputs {
  /** completion is 0-1 when known; undefined means the backend only reports a view. */
  watched: Map<
    string,
    { completion?: number; viewCount: number; lastViewedAt?: number }
  >;
  inProgress: Map<string, { progress: number; lastViewedAt?: number }>;
  /**
   * False when the in-progress source was unavailable this run. Negative
   * labels inferred from the ABSENCE of progress (abandoned, ignored) are
   * suppressed, because an empty map would be indistinguishable from an
   * outage.
   */
  inProgressAvailable: boolean;
  now: number;
}

export interface OutcomeChange {
  recommendationId: string;
  canonicalId: string;
  status:
    | RecommendationStatus.Watched
    | RecommendationStatus.Abandoned
    | RecommendationStatus.Ignored;
  reason: string;
}

/**
 * Label outcomes for open (notified) recommendations from polled state.
 *
 * - watched: seen in watch history at/above the completion threshold, or with
 *   a completed view when the backend reports no progress data.
 * - abandoned: started but stalled below the threshold and no longer being watched.
 * - ignored: the ignore window elapsed with no engagement at all.
 *
 * These passive labels are bookkeeping for cooldowns and the UI only. Explicit
 * good-pick/not-for-me feedback is the separate preference signal.
 */
export function decideOutcomes(
  open: RecommendationData[],
  inputs: OutcomeInputs,
): OutcomeChange[] {
  const changes: OutcomeChange[] = [];

  for (const rec of open) {
    if (rec.status !== RecommendationStatus.Notified) continue;
    const { canonicalId } = rec;
    const history = inputs.watched.get(canonicalId);
    const deliveredAt = rec.notifiedAt ?? rec.recommendedAt;
    const rawProgress = inputs.inProgress.get(canonicalId);
    const progress =
      rawProgress &&
      (rawProgress.lastViewedAt === undefined ||
        rawProgress.lastViewedAt >= deliveredAt)
        ? rawProgress
        : undefined;
    const engagementAfterDelivery =
      history !== undefined &&
      (history.lastViewedAt === undefined || history.lastViewedAt >= deliveredAt);
    // Watched: completion at/above threshold, or a view with no progress data.
    if (
      engagementAfterDelivery &&
      history &&
      (history.completion === undefined
        ? rec.mediaType === "movie" && history.viewCount >= 1
        : history.completion >= WATCHED_COMPLETION_THRESHOLD)
    ) {
      changes.push({
        recommendationId: rec.recommendationId,
        canonicalId,
        status: RecommendationStatus.Watched,
        reason:
          history.completion === undefined
            ? `viewCount=${history.viewCount}`
            : `completion=${history.completion.toFixed(2)}`,
      });
      continue;
    }

    // Abandoned: they started it but bailed below the threshold and are no
    // longer actively watching it. Removing an Arr entry is operational state,
    // not trustworthy user feedback, so it is deliberately ignored here.
    const startedButBailed =
      inputs.inProgressAvailable &&
      engagementAfterDelivery &&
      history !== undefined &&
      history.completion !== undefined &&
      history.completion < WATCHED_COMPLETION_THRESHOLD &&
      history.lastViewedAt !== undefined &&
      inputs.now - history.lastViewedAt >= ABANDONED_INACTIVITY_MS &&
      progress === undefined;
    if (startedButBailed) {
      changes.push({
        recommendationId: rec.recommendationId,
        canonicalId,
        status: RecommendationStatus.Abandoned,
        reason: `stalled at ${((history?.completion ?? 0) * 100).toFixed(0)}%`,
      });
      continue;
    }

    // Still actively in progress: leave open regardless of age. When the
    // in-progress source is unavailable we cannot rule active watching out,
    // so the ignored label is suppressed too.
    if (progress !== undefined || !inputs.inProgressAvailable) continue;

    // Ignored: no engagement within the window.
    const notifiedAt = rec.notifiedAt ?? rec.recommendedAt;
    if (inputs.now - notifiedAt > IGNORE_WINDOW_MS) {
      changes.push({
        recommendationId: rec.recommendationId,
        canonicalId,
        status: RecommendationStatus.Ignored,
        reason: "no engagement within 30 days",
      });
    }
  }

  return changes;
}
