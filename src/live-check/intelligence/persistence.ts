import { Entity } from "@micthiesen/mitools/entities";
import type {
  LivestreamFeedbackData,
  LivestreamFeedbackVerdict,
  LivestreamIntelligenceData,
} from "./types.js";

export const LivestreamIntelligenceEntity = new Entity<
  LivestreamIntelligenceData,
  ["streamerId"]
>("livestream-intelligence", ["streamerId"]);

export const LivestreamFeedbackEntity = new Entity<
  LivestreamFeedbackData,
  ["feedbackId"]
>("livestream-feedback", ["feedbackId"]);

export function getLivestreamIntelligence(
  streamerId: string,
): LivestreamIntelligenceData | undefined {
  return LivestreamIntelligenceEntity.get({ streamerId });
}

export function getAllLivestreamIntelligence(): LivestreamIntelligenceData[] {
  return LivestreamIntelligenceEntity.getAll();
}

export function saveLivestreamIntelligence(data: LivestreamIntelligenceData): void {
  LivestreamIntelligenceEntity.upsert(data);
}

export function clearLivestreamIntelligence(streamerId: string): void {
  LivestreamIntelligenceEntity.delete({ streamerId });
}

export function recordLivestreamFeedback(input: {
  streamerId: string;
  alertId: string;
  verdict: LivestreamFeedbackVerdict;
  note?: string;
}): LivestreamFeedbackData | undefined {
  const intelligence = getLivestreamIntelligence(input.streamerId);
  const alert = intelligence?.latestAlert;
  if (!alert || alert.alertId !== input.alertId) return undefined;
  const feedback: LivestreamFeedbackData = {
    feedbackId: input.alertId,
    streamerId: input.streamerId,
    alertId: input.alertId,
    alertType: alert.type,
    verdict: input.verdict,
    note: input.note?.trim() || undefined,
    createdAt: Date.now(),
  };
  LivestreamFeedbackEntity.upsert(feedback);
  return feedback;
}

export function buildLivestreamFeedbackDigest(limit = 20): string {
  return LivestreamFeedbackEntity.getAll()
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, limit)
    .map(
      (item) =>
        `${item.alertType}: ${item.verdict}${item.note ? ` (${item.note})` : ""}`,
    )
    .join("\n");
}
