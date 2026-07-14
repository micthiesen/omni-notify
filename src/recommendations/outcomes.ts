import { type RecommendationData, RecommendationStatus } from "./persistence.js";

/** Fraction of runtime that counts as actually having watched a title. */
export const WATCHED_COMPLETION_THRESHOLD = 0.8;

/** Days after notification with no engagement before a rec counts as ignored. */
export const IGNORE_WINDOW_MS = 30 * 24 * 60 * 60 * 1000;

export interface OutcomeInputs {
  /** completion is 0-1 when known; undefined means the backend only reports a view. */
  watched: Map<string, { completion?: number; viewCount: number }>;
  inProgress: Map<string, { progress: number }>;
  watchlistIds: Set<string>;
  /**
   * False when the in-progress source was unavailable this run. Negative
   * labels inferred from the ABSENCE of progress (abandoned, ignored) are
   * suppressed, because an empty map would be indistinguishable from an
   * outage.
   */
  inProgressAvailable: boolean;
  /**
   * False when any watchlist item failed identity resolution. Absence from
   * watchlistIds is then meaningless, so watchlist-removal inference is
   * suppressed.
   */
  watchlistComplete: boolean;
  now: number;
}

export interface OutcomeChange {
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
 * - abandoned: started but stalled below the threshold and no longer being
 *   watched, or removed from the watchlist without being watched.
 * - ignored: the ignore window elapsed with no engagement at all.
 *
 * These labels are bookkeeping for cooldowns and the UI only — they are never
 * fed back into taste/profile inputs (which derive solely from ground-truth
 * watch history).
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
    const progress = inputs.inProgress.get(canonicalId);
    const onWatchlist = inputs.watchlistIds.has(canonicalId);

    // Watched: completion at/above threshold, or a view with no progress data.
    if (
      history &&
      (history.completion === undefined
        ? history.viewCount >= 1
        : history.completion >= WATCHED_COMPLETION_THRESHOLD)
    ) {
      changes.push({
        canonicalId,
        status: RecommendationStatus.Watched,
        reason:
          history.completion === undefined
            ? `viewCount=${history.viewCount}`
            : `completion=${history.completion.toFixed(2)}`,
      });
      continue;
    }

    // Abandoned: they started it but bailed (below threshold, not actively
    // in progress anymore), or actively removed it from the watchlist. Both
    // are absence-based inferences, so they require trustworthy inputs.
    const startedButBailed =
      inputs.inProgressAvailable &&
      history !== undefined &&
      history.completion !== undefined &&
      history.completion < WATCHED_COMPLETION_THRESHOLD &&
      progress === undefined;
    const removedUnwatched =
      inputs.watchlistComplete &&
      rec.watchlistResult === "added" &&
      !onWatchlist &&
      history === undefined;
    if (startedButBailed || removedUnwatched) {
      changes.push({
        canonicalId,
        status: RecommendationStatus.Abandoned,
        reason: startedButBailed
          ? `stalled at ${((history?.completion ?? 0) * 100).toFixed(0)}%`
          : "removed from watchlist unwatched",
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
        canonicalId,
        status: RecommendationStatus.Ignored,
        reason: "no engagement within 30 days",
      });
    }
  }

  return changes;
}
