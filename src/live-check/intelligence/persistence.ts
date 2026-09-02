import { randomUUID } from "node:crypto";
import type { Docstore } from "@micthiesen/mitools/docstore";
import { Entity } from "@micthiesen/mitools/entities";
import { Clock, Effect, Option } from "effect";
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

export function getLivestreamIntelligence(streamerId: string) {
  return LivestreamIntelligenceEntity.get({ streamerId }).pipe(
    Effect.map(Option.getOrUndefined),
  );
}

export function saveLivestreamIntelligence(data: LivestreamIntelligenceData) {
  return LivestreamIntelligenceEntity.upsert(data);
}

export function clearLivestreamIntelligence(streamerId: string) {
  return LivestreamIntelligenceEntity.delete({ streamerId }).pipe(Effect.asVoid);
}

export function getLivestreamDiagnostics(streamerId: string) {
  return LivestreamDiagnosticsEntity.get({ streamerId }).pipe(
    Effect.map(Option.getOrUndefined),
  );
}

export function updateLivestreamStage(
  streamerId: string,
  sessionStartedAt: number | undefined,
  stage: LivestreamPipelineStage,
  value: LivestreamStageDiagnostic,
) {
  return Effect.gen(function* () {
    const previous = yield* getLivestreamDiagnostics(streamerId);
    const sameSession = previous?.sessionStartedAt === sessionStartedAt;
    const next: LivestreamDiagnosticsData = {
      streamerId,
      sessionStartedAt,
      stages: { ...(sameSession ? previous?.stages : {}), [stage]: value },
      updatedAt: yield* Clock.currentTimeMillis,
    };
    yield* LivestreamDiagnosticsEntity.upsert(next);
    return next;
  });
}

export type RecordLivestreamEventInput = Omit<
  LivestreamIntelligenceEventData,
  "eventId" | "createdAt"
> & { eventId?: string; createdAt?: number };

export function recordLivestreamEvent(input: RecordLivestreamEventInput) {
  return Effect.gen(function* () {
    const event: LivestreamIntelligenceEventData = {
      ...input,
      eventId: input.eventId ?? randomUUID(),
      createdAt: input.createdAt ?? (yield* Clock.currentTimeMillis),
    };
    yield* LivestreamIntelligenceEventEntity.upsert(event);
    if ((yield* LivestreamIntelligenceEventEntity.count()) > MAX_TIMELINE_EVENTS) {
      const oldest = (yield* LivestreamIntelligenceEventEntity.getAll())
        .sort((a, b) => a.createdAt - b.createdAt)
        .slice(0, PRUNE_BATCH_SIZE);
      yield* Effect.forEach(
        oldest,
        (item) => LivestreamIntelligenceEventEntity.delete({ eventId: item.eventId }),
        { discard: true },
      );
    }
    return event;
  });
}

export function getLivestreamEvents(streamerId?: string, limit = 100) {
  return LivestreamIntelligenceEventEntity.getAll().pipe(
    Effect.map((events) =>
      events
        .filter((event) => !streamerId || event.streamerId === streamerId)
        .sort((a, b) => b.createdAt - a.createdAt)
        .slice(0, Math.max(1, Math.min(limit, 200))),
    ),
  );
}

export function getLatestDestinyConfirmation(
  streamerId: string,
  sessionStartedAt: number,
) {
  return LivestreamIntelligenceEventEntity.getAll().pipe(
    Effect.map(
      (events) =>
        events
          .filter(
            (event) =>
              event.streamerId === streamerId &&
              event.sessionStartedAt === sessionStartedAt &&
              event.kind === "voice" &&
              event.status === "success" &&
              event.title === DESTINY_CONFIRMED_EVENT_TITLE,
          )
          .sort((left, right) => right.createdAt - left.createdAt)[0],
    ),
  );
}

export function recordLivestreamFeedback(input: {
  streamerId: string;
  alertId: string;
  verdict: LivestreamFeedbackVerdict;
  note?: string;
}): Effect.Effect<LivestreamFeedbackData | undefined, unknown, Docstore> {
  return Effect.gen(function* () {
    const intelligence = yield* getLivestreamIntelligence(input.streamerId);
    const alert = intelligence?.latestAlert;
    if (!alert || alert.alertId !== input.alertId) return undefined;
    const feedback: LivestreamFeedbackData = {
      feedbackId: input.alertId,
      streamerId: input.streamerId,
      alertId: input.alertId,
      alertType: alert.type,
      verdict: input.verdict,
      note: input.note?.trim() || undefined,
      createdAt: yield* Clock.currentTimeMillis,
    };
    yield* LivestreamFeedbackEntity.upsert(feedback);
    yield* recordLivestreamEvent({
      streamerId: input.streamerId,
      sessionStartedAt: intelligence.sessionStartedAt,
      kind: "feedback",
      status: "info",
      title: `Alert marked ${input.verdict.replaceAll("_", " ")}`,
      detail: input.note?.trim() || undefined,
      metrics: { alertType: alert.type },
    }).pipe(Effect.ignore);
    return feedback;
  });
}

export function buildLivestreamFeedbackDigest(limit = 20) {
  return LivestreamFeedbackEntity.getAll().pipe(
    Effect.map((items) =>
      items
        .sort((a, b) => b.createdAt - a.createdAt)
        .slice(0, limit)
        .map(
          (item) =>
            `${item.alertType}: ${item.verdict}${item.note ? ` (${item.note})` : ""}`,
        )
        .join("\n"),
    ),
  );
}
