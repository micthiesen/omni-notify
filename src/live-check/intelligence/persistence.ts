import { randomUUID } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import type {
  LivestreamDiagnosticsData,
  LivestreamFeedbackData,
  LivestreamFeedbackVerdict,
  LivestreamIntelligenceData,
  LivestreamIntelligenceEventData,
  LivestreamPipelineStage,
  LivestreamStageDiagnostic,
} from "./types.js";

const MAX_TIMELINE_EVENTS = 3_000;
const PRUNE_BATCH_SIZE = 250;
export const DESTINY_CONFIRMED_EVENT_TITLE = "Destiny confirmed as a live participant";

export const LivestreamIntelligenceEntity = new Entity<
  LivestreamIntelligenceData,
  ["streamerId"]
>("livestream-intelligence", ["streamerId"]);

export const LivestreamFeedbackEntity = new Entity<
  LivestreamFeedbackData,
  ["feedbackId"]
>("livestream-feedback", ["feedbackId"]);

export const LivestreamDiagnosticsEntity = new Entity<
  LivestreamDiagnosticsData,
  ["streamerId"]
>("livestream-diagnostics", ["streamerId"]);

export const LivestreamIntelligenceEventEntity = new Entity<
  LivestreamIntelligenceEventData,
  ["eventId"]
>("livestream-intelligence-event", ["eventId"]);

export function getLivestreamIntelligence(
  streamerId: string,
): LivestreamIntelligenceData | undefined {
  return LivestreamIntelligenceEntity.get({ streamerId });
}

export function saveLivestreamIntelligence(data: LivestreamIntelligenceData): void {
  LivestreamIntelligenceEntity.upsert(data);
}

export function clearLivestreamIntelligence(streamerId: string): void {
  LivestreamIntelligenceEntity.delete({ streamerId });
}

export function getLivestreamDiagnostics(
  streamerId: string,
): LivestreamDiagnosticsData | undefined {
  return LivestreamDiagnosticsEntity.get({ streamerId });
}

export function updateLivestreamStage(
  streamerId: string,
  sessionStartedAt: number | undefined,
  stage: LivestreamPipelineStage,
  value: LivestreamStageDiagnostic,
): LivestreamDiagnosticsData {
  const previous = getLivestreamDiagnostics(streamerId);
  const sameSession = previous?.sessionStartedAt === sessionStartedAt;
  const next: LivestreamDiagnosticsData = {
    streamerId,
    sessionStartedAt,
    stages: {
      ...(sameSession ? previous?.stages : {}),
      [stage]: value,
    },
    updatedAt: Date.now(),
  };
  LivestreamDiagnosticsEntity.upsert(next);
  return next;
}

export type RecordLivestreamEventInput = Omit<
  LivestreamIntelligenceEventData,
  "eventId" | "createdAt"
> & {
  eventId?: string;
  createdAt?: number;
};

export function recordLivestreamEvent(
  input: RecordLivestreamEventInput,
): LivestreamIntelligenceEventData {
  const event: LivestreamIntelligenceEventData = {
    ...input,
    eventId: input.eventId ?? randomUUID(),
    createdAt: input.createdAt ?? Date.now(),
  };
  LivestreamIntelligenceEventEntity.upsert(event);
  if (LivestreamIntelligenceEventEntity.count() > MAX_TIMELINE_EVENTS) {
    const oldest = LivestreamIntelligenceEventEntity.getAll()
      .sort((a, b) => a.createdAt - b.createdAt)
      .slice(0, PRUNE_BATCH_SIZE);
    for (const item of oldest) {
      LivestreamIntelligenceEventEntity.delete({ eventId: item.eventId });
    }
  }
  return event;
}

export function getLivestreamEvents(
  streamerId?: string,
  limit = 100,
): LivestreamIntelligenceEventData[] {
  return LivestreamIntelligenceEventEntity.getAll()
    .filter((event) => !streamerId || event.streamerId === streamerId)
    .sort((a, b) => b.createdAt - a.createdAt)
    .slice(0, Math.max(1, Math.min(limit, 200)));
}

export function getLatestDestinyConfirmation(
  streamerId: string,
  sessionStartedAt: number,
): LivestreamIntelligenceEventData | undefined {
  return LivestreamIntelligenceEventEntity.getAll()
    .filter(
      (event) =>
        event.streamerId === streamerId &&
        event.sessionStartedAt === sessionStartedAt &&
        event.kind === "voice" &&
        event.status === "success" &&
        event.title === DESTINY_CONFIRMED_EVENT_TITLE,
    )
    .sort((left, right) => right.createdAt - left.createdAt)[0];
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
  try {
    recordLivestreamEvent({
      streamerId: input.streamerId,
      sessionStartedAt: intelligence.sessionStartedAt,
      kind: "feedback",
      status: "info",
      title: `Alert marked ${input.verdict.replaceAll("_", " ")}`,
      detail: input.note?.trim() || undefined,
      metrics: { alertType: alert.type },
    });
  } catch {
    // Feedback is the durable user action; optional observability must never
    // make a successfully stored correction look like a failed request.
  }
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
