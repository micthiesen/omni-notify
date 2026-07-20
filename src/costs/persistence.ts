import { randomUUID } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import { Logger } from "@micthiesen/mitools/logging";
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

const logger = new Logger("Costs");

export type RecordCostEventInput = Omit<
  CostEventData,
  "eventId" | "incurredAt" | "runId"
> & {
  eventId?: string;
  incurredAt?: number;
  runId?: string;
};

export function recordCostEvent(input: RecordCostEventInput): CostEventData {
  const context = getCurrentRunContext();
  const event: CostEventData = {
    ...input,
    eventId: input.eventId ?? randomUUID(),
    incurredAt: input.incurredAt ?? Date.now(),
    runId: input.runId ?? context?.runId,
  };
  CostEventEntity.upsert(event);
  return event;
}

/** Cost telemetry must never turn a successful paid provider call into a retry. */
export function recordCostEventSafely(
  input: RecordCostEventInput,
): CostEventData | undefined {
  try {
    return recordCostEvent(input);
  } catch (error) {
    logger.error("Failed to persist cost event", error);
    return undefined;
  }
}

export function getCostEvents(): CostEventData[] {
  return CostEventEntity.getAll();
}

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
