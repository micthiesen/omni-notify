import { digest } from "../../utils/fingerprint.js";
import type { ListenedEpisode } from "../account.js";
import type { PodcastRecommendationData } from "../persistence.js";
import type { PodcastTasteEvidenceData } from "./types.js";

export { fingerprintEvidence } from "../../utils/fingerprint.js";

export function normalizeShowKey(showTitle: string): string {
  return showTitle.trim().toLowerCase();
}

export function deriveListenEvidence(
  listened: ListenedEpisode[],
): PodcastTasteEvidenceData[] {
  return listened.map((item) => {
    const identity = [
      normalizeShowKey(item.showTitle),
      item.episodeGuid ?? item.episodeTitle,
      item.listenedAt,
      item.completion === undefined ? "unknown" : item.completion.toFixed(3),
      item.starred === true,
    ].join(":");
    return {
      evidenceId: `listen:${digest(identity)}`,
      kind: "listen",
      showKey: normalizeShowKey(item.showTitle),
      showTitle: item.showTitle,
      episodeTitle: item.episodeTitle,
      observedAt: item.listenedAt,
      completion: item.completion,
      starred: item.starred,
    };
  });
}

export function deriveRecommendationEvidence(
  recommendations: PodcastRecommendationData[],
): PodcastTasteEvidenceData[] {
  const evidence: PodcastTasteEvidenceData[] = [];
  for (const recommendation of recommendations) {
    const fields = recommendationFields(recommendation);
    const observedAt =
      recommendation.resolvedAt ??
      recommendation.notifiedAt ??
      recommendation.recommendedAt;
    evidence.push({
      evidenceId: `recommendation:${digest(
        [
          recommendation.recommendationId,
          "outcome",
          recommendation.status,
          observedAt,
          JSON.stringify(fields),
        ].join(":"),
      )}`,
      kind: "recommendation_outcome",
      ...fields,
      observedAt,
      recommendationStatus: recommendation.status,
    });
    if (recommendation.feedback) {
      const feedbackObservedAt =
        recommendation.feedbackAt ?? recommendation.recommendedAt;
      evidence.push({
        evidenceId: `recommendation:${digest(
          [
            recommendation.recommendationId,
            "feedback",
            recommendation.feedback,
            feedbackObservedAt,
            JSON.stringify(fields),
          ].join(":"),
        )}`,
        kind: "explicit_feedback",
        ...fields,
        observedAt: feedbackObservedAt,
        feedback: recommendation.feedback,
      });
    }
  }
  return evidence;
}

function recommendationFields(recommendation: PodcastRecommendationData) {
  return {
    showKey: normalizeShowKey(recommendation.showTitle),
    showTitle: recommendation.showTitle,
    episodeTitle: recommendation.episodeTitle,
    recommendationId: recommendation.recommendationId,
    discoveredVia: recommendation.discoveredVia,
    matchedVoices: recommendation.matchedVoices,
    durationMinutes: recommendation.durationMinutes,
  };
}
