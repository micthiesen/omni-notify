import { anthropic } from "@ai-sdk/anthropic";
import { google } from "@ai-sdk/google";
import { openai } from "@ai-sdk/openai";
import type { LanguageModel } from "ai";
import { createProviderRegistry } from "ai";
import config from "../utils/config.js";

type RegisteredModelId = Parameters<typeof modelRegistry.languageModel>[0];

export const modelRegistry = createProviderRegistry({ anthropic, google, openai });

export function getBriefingModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.BRIEFING_MODEL, "google:gemini-3.5-flash");
}

export function getExtractionModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.EXTRACTION_MODEL, "google:gemini-3.1-flash-lite");
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
  return resolveModel(config.CALENDAR_EXTRACTION_MODEL, "google:gemini-3.5-flash");
}

/** Cheap shared relevance classifier that gates both email pipelines. */
export function getTriageModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.TRIAGE_MODEL, "google:gemini-3.1-flash-lite");
}

export function getRecsShortlistModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.RECS_SHORTLIST_MODEL, "openai:gpt-5.6-luna");
}

export function getRecsSelectionModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.RECS_SELECTION_MODEL, "openai:gpt-5.6");
}

export function getTasteReflectionModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.TASTE_REFLECTION_MODEL, "openai:gpt-5.6-luna");
}

export function getPodcastTasteReflectionModel(): {
  model: LanguageModel;
  modelId: string;
} {
  return resolveModel(config.PODCAST_TASTE_REFLECTION_MODEL, "openai:gpt-5.6-luna");
}

/**
 * PressPods metadata extraction rates every retriever's result (up to 7 calls
 * per episode) but the calls are small; cleaning is one large rewrite pass.
 */
export function getPressPodsMetadataModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.PRESSPODS_METADATA_MODEL, "google:gemini-3.5-flash");
}

export function getPressPodsCleaningModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.PRESSPODS_CLEANING_MODEL, "google:gemini-3.5-flash");
}

function resolveModel(configured: string | undefined, fallback: RegisteredModelId) {
  const modelId = (configured ?? fallback) as RegisteredModelId;
  return { model: modelRegistry.languageModel(modelId), modelId };
}
