import { digest } from "../../utils/fingerprint.js";
import type { RecommendationData } from "../persistence.js";
import type { CanonicalWatchObservation, TasteEvidenceData } from "./types.js";

export { fingerprintEvidence } from "../../utils/fingerprint.js";

export function deriveWatchEvidence(
  observations: CanonicalWatchObservation[],
): TasteEvidenceData[] {
  return observations.map(({ canonicalId, item, metadata }) => {
    const identity = [
      canonicalId,
      item.viewedAt,
      item.viewCount,
      item.completion === undefined ? "unknown" : item.completion.toFixed(3),
      JSON.stringify(metadata ?? {}),
    ].join(":");
    return {
      evidenceId: `plex:${digest(identity)}`,
      kind: "plex_watch",
      canonicalId,
      title: item.title,
      year: item.year,
      mediaType: item.mediaType,
      observedAt: item.viewedAt,
      viewCount: item.viewCount,
      completion: item.completion,
      ...metadata,
    };
  });
}

export function deriveRecommendationEvidence(
  recommendations: RecommendationData[],
): TasteEvidenceData[] {
  const evidence: TasteEvidenceData[] = [];
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
      const observedAt = recommendation.feedbackAt ?? recommendation.recommendedAt;
      evidence.push({
        evidenceId: `recommendation:${digest(
          [
            recommendation.recommendationId,
            "feedback",
            recommendation.feedback,
            observedAt,
            JSON.stringify(fields),
          ].join(":"),
        )}`,
        kind: "explicit_feedback",
        ...fields,
        observedAt,
        feedback: recommendation.feedback,
      });
    }
  }
  return evidence;
}

function recommendationFields(recommendation: RecommendationData) {
  const extended = recommendation as RecommendationData & {
    startedAt?: number;
    source?: string;
  };
  return {
    canonicalId: recommendation.canonicalId as TasteEvidenceData["canonicalId"],
    title: recommendation.title,
    year: recommendation.year,
    mediaType: recommendation.mediaType,
    recommendationId: recommendation.recommendationId,
    recommendedAt: recommendation.recommendedAt,
    startedAt: extended.startedAt,
    source: extended.source,
    genres: recommendation.genres,
    runtimeMinutes: recommendation.runtimeMinutes,
    seasonCount: recommendation.seasonCount,
    episodeCount: recommendation.episodeCount,
    seriesStatus: recommendation.seriesStatus,
    originalLanguage: recommendation.originalLanguage,
    originCountries: recommendation.originCountries,
    creators: recommendation.creators,
    cast: recommendation.cast,
    keywords: recommendation.keywords,
    certification: recommendation.certification,
  };
}
