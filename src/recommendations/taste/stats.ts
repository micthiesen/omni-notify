import { RecommendationStatus } from "../persistence.js";
import { MediaType } from "../types.js";
import type { BehavioralStats, TasteEvidenceData } from "./types.js";

export function computeBehavioralStats(evidence: TasteEvidenceData[]): BehavioralStats {
  const latestWatch = latestBy(evidence.filter((item) => item.kind === "plex_watch"));
  const latestOutcome = latestRecommendationEvidence(
    evidence.filter((item) => item.kind === "recommendation_outcome"),
  );
  const latestFeedback = latestRecommendationEvidence(
    evidence.filter((item) => item.kind === "explicit_feedback"),
  );
  const deliveredRecommendationIds = new Set(
    [...latestOutcome.entries()]
      .filter(([, item]) => isDelivered(item.recommendationStatus))
      .map(([recommendationId]) => recommendationId),
  );

  const hoursToStart = [...latestOutcome.values()]
    .filter(
      (item) =>
        isDelivered(item.recommendationStatus) &&
        item.recommendationId &&
        item.startedAt !== undefined &&
        item.recommendedAt !== undefined &&
        item.startedAt >= item.recommendedAt,
    )
    .map((item) => (item.startedAt! - item.recommendedAt!) / (60 * 60 * 1000));

  const sourcePerformance: BehavioralStats["sourcePerformance"] = {};
  const recommendationSources = new Map<string, string>();
  for (const item of evidence) {
    if (item.recommendationId && item.source) {
      recommendationSources.set(item.recommendationId, item.source);
    }
  }
  for (const recommendationId of deliveredRecommendationIds) {
    const source = recommendationSources.get(recommendationId) ?? "unknown";
    if (!sourcePerformance[source]) {
      sourcePerformance[source] = {
        total: 0,
        watched: 0,
        goodPick: 0,
        notForMe: 0,
      };
    }
    const row = sourcePerformance[source];
    row.total++;
    const outcome = latestOutcome.get(recommendationId);
    const feedback = latestFeedback.get(recommendationId);
    if (outcome?.recommendationStatus === RecommendationStatus.Watched) row.watched++;
    if (feedback?.feedback === "good_pick") row.goodPick++;
    if (feedback?.feedback === "not_for_me") row.notForMe++;
  }

  return {
    completedMovies: [...latestWatch.values()].filter(
      (item) => item.mediaType === MediaType.Movie && isCompletedWatch(item),
    ).length,
    completedSeries: [...latestWatch.values()].filter(
      (item) => item.mediaType === MediaType.Tv && isCompletedWatch(item),
    ).length,
    rewatchedTitles: [...latestWatch.values()].filter(
      (item) => isCompletedWatch(item) && (item.viewCount ?? 1) > 1,
    ).length,
    recommendations: {
      total: deliveredRecommendationIds.size,
      watched: countStatus(latestOutcome, RecommendationStatus.Watched),
      abandoned: countStatus(latestOutcome, RecommendationStatus.Abandoned),
      ignored: countStatus(latestOutcome, RecommendationStatus.Ignored),
      failed: countStatus(latestOutcome, RecommendationStatus.Failed),
      awaitingOutcome: [...latestOutcome.entries()].filter(
        ([recommendationId, item]) =>
          item.recommendationStatus === RecommendationStatus.Notified &&
          latestFeedback.get(recommendationId)?.feedback !== "not_for_me" &&
          latestFeedback.get(recommendationId)?.feedback !== "already_watched",
      ).length,
    },
    feedback: {
      goodPick: countFeedback(latestFeedback, "good_pick"),
      notForMe: countFeedback(latestFeedback, "not_for_me"),
      alreadyWatched: countFeedback(latestFeedback, "already_watched"),
    },
    averageHoursToStart:
      hoursToStart.length > 0
        ? hoursToStart.reduce((sum, value) => sum + value, 0) / hoursToStart.length
        : undefined,
    sourcePerformance,
  };
}

function isDelivered(status: RecommendationStatus | undefined): boolean {
  return (
    status === RecommendationStatus.Notified ||
    status === RecommendationStatus.Watched ||
    status === RecommendationStatus.Abandoned ||
    status === RecommendationStatus.Ignored
  );
}

function latestBy(items: TasteEvidenceData[]): Map<string, TasteEvidenceData> {
  const latest = new Map<string, TasteEvidenceData>();
  for (const item of items) {
    const prior = latest.get(item.canonicalId);
    if (
      !prior ||
      item.observedAt > prior.observedAt ||
      (item.observedAt === prior.observedAt &&
        item.evidenceId.localeCompare(prior.evidenceId) > 0)
    )
      latest.set(item.canonicalId, item);
  }
  return latest;
}

function latestRecommendationEvidence(
  items: TasteEvidenceData[],
): Map<string, TasteEvidenceData> {
  const latest = new Map<string, TasteEvidenceData>();
  for (const item of items) {
    if (!item.recommendationId) continue;
    const prior = latest.get(item.recommendationId);
    if (
      !prior ||
      item.observedAt > prior.observedAt ||
      (item.observedAt === prior.observedAt &&
        (statusPrecedence(item) > statusPrecedence(prior) ||
          (statusPrecedence(item) === statusPrecedence(prior) &&
            item.evidenceId.localeCompare(prior.evidenceId) > 0)))
    ) {
      latest.set(item.recommendationId, item);
    }
  }
  return latest;
}

function isCompletedWatch(item: TasteEvidenceData): boolean {
  return item.completion === undefined
    ? item.mediaType === MediaType.Movie && (item.viewCount ?? 0) >= 1
    : item.completion >= 0.8;
}

function statusPrecedence(item: TasteEvidenceData): number {
  switch (item.recommendationStatus) {
    case RecommendationStatus.Watched:
    case RecommendationStatus.Abandoned:
    case RecommendationStatus.Ignored:
      return 4;
    case RecommendationStatus.Failed:
      return 3;
    case RecommendationStatus.Notified:
      return 2;
    case RecommendationStatus.Pending:
      return 1;
    default:
      return 0;
  }
}

function countStatus(
  evidence: Map<string, TasteEvidenceData>,
  status: RecommendationStatus,
): number {
  return [...evidence.values()].filter((item) => item.recommendationStatus === status)
    .length;
}

function countFeedback(
  evidence: Map<string, TasteEvidenceData>,
  feedback: NonNullable<TasteEvidenceData["feedback"]>,
): number {
  return [...evidence.values()].filter((item) => item.feedback === feedback).length;
}
