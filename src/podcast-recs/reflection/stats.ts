import { PodcastRecommendationStatus } from "../persistence.js";
import type { PodcastBehavioralStats, PodcastTasteEvidenceData } from "./types.js";

/**
 * Deterministic counts injected into the reflection prompt as ground truth so
 * the model is never asked to tally the ledger itself. Listen rows dedupe on
 * (show, episode) keeping the newest observation; recommendation rows dedupe
 * on recommendationId the same way.
 */
export function computePodcastBehavioralStats(
  evidence: PodcastTasteEvidenceData[],
): PodcastBehavioralStats {
  const latestListens = new Map<string, PodcastTasteEvidenceData>();
  const latestOutcomes = new Map<string, PodcastTasteEvidenceData>();
  const latestFeedback = new Map<string, PodcastTasteEvidenceData>();

  for (const item of evidence) {
    if (item.kind === "listen") {
      const key = `${item.showKey}#${item.episodeTitle ?? ""}`;
      const existing = latestListens.get(key);
      if (!existing || item.observedAt > existing.observedAt) {
        latestListens.set(key, item);
      }
    } else if (item.recommendationId) {
      const target =
        item.kind === "recommendation_outcome" ? latestOutcomes : latestFeedback;
      const existing = target.get(item.recommendationId);
      if (!existing || item.observedAt > existing.observedAt) {
        target.set(item.recommendationId, item);
      }
    }
  }

  const listens = [...latestListens.values()];
  const outcomes = [...latestOutcomes.values()];
  const feedback = [...latestFeedback.values()];
  const countStatus = (status: PodcastRecommendationStatus) =>
    outcomes.filter((item) => item.recommendationStatus === status).length;

  return {
    listenedEpisodes: listens.filter(
      (item) => item.completion === undefined || item.completion >= 0.8,
    ).length,
    startedEpisodes: listens.length,
    starredEpisodes: listens.filter((item) => item.starred === true).length,
    distinctShows: new Set(evidence.map((item) => item.showKey)).size,
    recommendations: {
      total: outcomes.length,
      listened: countStatus(PodcastRecommendationStatus.Listened),
      abandoned: countStatus(PodcastRecommendationStatus.Abandoned),
      ignored: countStatus(PodcastRecommendationStatus.Ignored),
      failed: countStatus(PodcastRecommendationStatus.Failed),
      awaitingOutcome:
        countStatus(PodcastRecommendationStatus.Pending) +
        countStatus(PodcastRecommendationStatus.Notified),
    },
    feedback: {
      goodPick: feedback.filter((item) => item.feedback === "good_pick").length,
      notForMe: feedback.filter((item) => item.feedback === "not_for_me").length,
    },
  };
}
