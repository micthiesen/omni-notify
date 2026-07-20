import { anthropic } from "@ai-sdk/anthropic";
import { google } from "@ai-sdk/google";
import { openai } from "@ai-sdk/openai";
import { createProviderRegistry, type LanguageModel, wrapLanguageModel } from "ai";
import { currentCostFeature, recordCostEventSafely } from "../costs/persistence.js";
import config from "../utils/config.js";
import { hasPrice, llmCostCents } from "./cost.js";

type RegisteredModelId = Parameters<typeof modelRegistry.languageModel>[0];

export const modelRegistry = createProviderRegistry({ anthropic, google, openai });

export function getBriefingModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(
    config.BRIEFING_MODEL,
    "google:gemini-3.5-flash",
    "briefings",
    "generate",
  );
}

export function getExtractionModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(
    config.EXTRACTION_MODEL,
    "google:gemini-3.1-flash-lite",
    "parcel-tracker",
    "extract-deliveries",
  );
}

/**
 * Calendar extraction runs on a stronger model than parcel extraction: every
 * serious calendar failure in production traced to flash-lite output
 * degeneration (repeated objects, field soup in timeZone), and triage keeps
 * call volume low enough to afford it.
 */
export function getCalendarExtractionModel(): {
  model: LanguageModel;
  modelId: string;
} {
  return resolveModel(
    config.CALENDAR_EXTRACTION_MODEL,
    "google:gemini-3.5-flash",
    "calendar-events",
    "extract-events",
  );
}

/** Cheap shared relevance classifier that gates both email pipelines. */
export function getTriageModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(
    config.TRIAGE_MODEL,
    "google:gemini-3.1-flash-lite",
    "email-triage",
    "classify",
  );
}

export function getRecsShortlistModel(operation = "shortlist"): {
  model: LanguageModel;
  modelId: string;
} {
  return resolveModel(
    config.RECS_SHORTLIST_MODEL,
    "openai:gpt-5.6-luna",
    "media-recommendations",
    operation,
  );
}

export function getRecsSelectionModel(operation = "select"): {
  model: LanguageModel;
  modelId: string;
} {
  return resolveModel(
    config.RECS_SELECTION_MODEL,
    "openai:gpt-5.6",
    "media-recommendations",
    operation,
  );
}

export function getTasteReflectionModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(
    config.TASTE_REFLECTION_MODEL,
    "openai:gpt-5.6-luna",
    "media-recommendations",
    "taste-reflection",
  );
}

export function getPodcastTasteReflectionModel(): {
  model: LanguageModel;
  modelId: string;
} {
  return resolveModel(
    config.PODCAST_TASTE_REFLECTION_MODEL,
    "openai:gpt-5.6-luna",
    "podcast-recommendations",
    "taste-reflection",
  );
}

/**
 * PressPods metadata extraction rates every retriever's result (up to 7 calls
 * per episode) but the calls are small; cleaning is one large rewrite pass.
 */
export function getPressPodsMetadataModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(
    config.PRESSPODS_METADATA_MODEL,
    "google:gemini-3.5-flash",
    "press-pods",
    "rate-retrieval",
  );
}

export function getPressPodsCleaningModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(
    config.PRESSPODS_CLEANING_MODEL,
    "google:gemini-3.5-flash",
    "press-pods",
    "clean-narration",
  );
}

function resolveModel(
  configured: string | undefined,
  fallback: RegisteredModelId,
  feature: string,
  operation: string,
) {
  const modelId = (configured ?? fallback) as RegisteredModelId;
  const [service = "unknown", bareModel = modelId] = modelId.split(":", 2);
  const model = wrapLanguageModel({
    model: modelRegistry.languageModel(modelId),
    middleware: {
      specificationVersion: "v4",
      wrapGenerate: async ({ doGenerate }) => {
        const result = await doGenerate();
        const inputTokens = result.usage.inputTokens.total ?? 0;
        const outputTokens = result.usage.outputTokens.total ?? 0;
        recordCostEventSafely({
          category: "llm",
          feature: currentCostFeature(feature),
          operation,
          service,
          model: bareModel,
          costCents: hasPrice(modelId)
            ? llmCostCents(modelId, { inputTokens, outputTokens })
            : null,
          priceStatus: hasPrice(modelId) ? "estimated" : "unknown",
          usage: {
            inputTokens,
            inputNoCacheTokens: result.usage.inputTokens.noCache ?? 0,
            cacheReadTokens: result.usage.inputTokens.cacheRead ?? 0,
            cacheWriteTokens: result.usage.inputTokens.cacheWrite ?? 0,
            outputTokens,
            reasoningTokens: result.usage.outputTokens.reasoning ?? 0,
            requests: 1,
          },
        });
        return result;
      },
    },
  });
  return { model, modelId };
}
