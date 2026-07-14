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

export function getRecsShortlistModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.RECS_SHORTLIST_MODEL, "openai:gpt-5-mini");
}

export function getRecsSelectionModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.RECS_SELECTION_MODEL, "openai:gpt-5");
}

function resolveModel(configured: string | undefined, fallback: RegisteredModelId) {
  const modelId = (configured ?? fallback) as RegisteredModelId;
  return { model: modelRegistry.languageModel(modelId), modelId };
}
