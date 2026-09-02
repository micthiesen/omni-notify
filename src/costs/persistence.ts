import { randomUUID } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import { Logger } from "@micthiesen/mitools/logging";
import { Clock, Effect } from "effect";
import { getCurrentRunContext } from "../task-runs/logCapture.js";

export type CostCategory = "llm" | "search" | "tts" | "retrieval" | "transcription";
export type CostPriceStatus = "priced" | "estimated" | "free" | "unknown";

export interface CostUsage {
  inputTokens?: number;
  inputNoCacheTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  outputTokens?: number;
  reasoningTokens?: number;
  characters?: number;
  requests?: number;
  credits?: number;
}

export interface CostEventData {
  eventId: string;
  incurredAt: number;
  category: CostCategory;
  feature: string;
  operation: string;
  service: string;
  model?: string;
  costCents: number | null;
  priceStatus: CostPriceStatus;
  usage: CostUsage;
  runId?: string;
}

export const CostEventEntity = new Entity<CostEventData, ["eventId"]>("cost-event", [
  "eventId",
]);

const logger = Logger.named("Costs");

export type RecordCostEventInput = Omit<
  CostEventData,
  "eventId" | "incurredAt" | "runId"
> & {
  eventId?: string;
  incurredAt?: number;
  runId?: string;
};

export const recordCostEvent = Effect.fn("Costs.record")(function* (
  input: RecordCostEventInput,
) {
  const context = getCurrentRunContext();
  const now = yield* Clock.currentTimeMillis;
  const event: CostEventData = {
    ...input,
    eventId: input.eventId ?? randomUUID(),
    incurredAt: input.incurredAt ?? now,
    runId: input.runId ?? context?.runId,
  };
  yield* CostEventEntity.upsert(event);
  return event;
});

/** Cost telemetry must never turn a successful paid provider call into a retry. */
export const recordCostEventSafely = Effect.fn("Costs.recordSafely")(function* (
  input: RecordCostEventInput,
) {
  return yield* recordCostEvent(input).pipe(
    Effect.catch((error) =>
      logger.error("Failed to persist cost event", error).pipe(Effect.as(undefined)),
    ),
  );
});

export const getCostEvents = Effect.fn("Costs.getAll")(function* () {
  return yield* CostEventEntity.getAll();
});

/** Prefer runtime attribution, while stable hints cover calls outside task runs. */
export function currentCostFeature(fallback: string): string {
  const name = getCurrentRunContext()?.taskName.toLowerCase();
  if (!name) return fallback;
  if (name.includes("presspods")) return "press-pods";
  if (name.includes("podcast")) return "podcast-recommendations";
  if (name.includes("recommendation") || name.includes("taste")) {
    return "media-recommendations";
  }
  if (name.includes("parcel")) return "parcel-tracker";
  if (name.includes("calendar")) return "calendar-events";
  if (name.includes("briefing")) return "briefings";
  return fallback;
}
