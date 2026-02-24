import { anthropic } from "@ai-sdk/anthropic";
import { google } from "@ai-sdk/google";
import { openai } from "@ai-sdk/openai";
import type { LanguageModel } from "ai";
import { createProviderRegistry } from "ai";
import config from "../utils/config.js";

type RegisteredModelId = Parameters<typeof modelRegistry.languageModel>[0];

export const modelRegistry = createProviderRegistry({ anthropic, google, openai });

export function getBriefingModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.BRIEFING_MODEL, "google:gemini-3-pro-preview");
}

export function getFilterModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.FILTER_MODEL, "google:gemini-3-flash-preview");
}

export function getExtractionModel(): { model: LanguageModel; modelId: string } {
  return resolveModel(config.EXTRACTION_MODEL, "google:gemini-3-flash-preview");
}

function resolveModel(configured: string | undefined, fallback: RegisteredModelId) {
  const modelId = (configured ?? fallback) as RegisteredModelId;
  return { model: modelRegistry.languageModel(modelId), modelId };
}
