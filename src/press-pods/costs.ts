import { DefaultMap } from "@micthiesen/mitools/collections";
import { Logger } from "@micthiesen/mitools/logging";

export interface CompletionUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface Costs {
  llmCents: number;
  ttsCents: number;
  detailCents: Record<string, number>;
  detailTokens: Record<string, { input: number; output: number }>;
  detailChars: Record<string, number>;
}

const LOGGER = new Logger("PressPods.CostCounter");

// Prices are in USD cents per token/character. Unlisted models cost 0 (with a
// warning); extend the tables when switching models.
// https://ai.google.dev/pricing / https://mistral.ai/pricing
const llmInputTokenCents: Record<string, number> = {
  "gemini-3-flash-preview": 0.00005, // $0.50 per 1M tokens
  "gemini-3.5-flash": 0.00005, // $0.50 per 1M tokens
  "gemini-3.1-flash-lite": 0.00001, // $0.10 per 1M tokens
};
const llmOutputTokenCents: Record<string, number> = {
  "gemini-3-flash-preview": 0.0003, // $3.00 per 1M tokens
  "gemini-3.5-flash": 0.0003, // $3.00 per 1M tokens
  "gemini-3.1-flash-lite": 0.00004, // $0.40 per 1M tokens
};
const ttsCharacterCents: Record<string, number> = {
  "voxtral-mini-tts-2603": 0.0016, // Mistral Voxtral ($0.016 / 1k chars)
};

export default class CostCounter {
  private llmCents = 0;
  private ttsCents = 0;
  private detailCents = new DefaultMap<string, number>(() => 0);
  private detailTokens = new DefaultMap<string, { input: number; output: number }>(
    () => ({ input: 0, output: 0 }),
  );
  private detailChars = new DefaultMap<string, number>(() => 0);

  public getCosts(): Costs {
    return {
      llmCents: this.llmCents,
      ttsCents: this.ttsCents,
      detailCents: this.detailCents.toObject(),
      detailTokens: this.detailTokens.toObject(),
      detailChars: this.detailChars.toObject(),
    };
  }

  public recordLlmUsage(
    model: string,
    fn: string,
    usage: CompletionUsage | undefined,
  ): void {
    if (!usage) {
      LOGGER.warn("No usage data for LLM", { model });
      return;
    }
    const bareModel = model.split(":").pop() ?? model;
    if (llmInputTokenCents[bareModel] === undefined) {
      LOGGER.debug(`No pricing for model ${bareModel}; counting as $0`);
    }
    const inputCents = (llmInputTokenCents[bareModel] ?? 0) * usage.promptTokens;
    const outputCents = (llmOutputTokenCents[bareModel] ?? 0) * usage.completionTokens;
    this.llmCents += inputCents + outputCents;

    this.recordDetailCents(`${bareModel}-${fn}-input`, inputCents);
    this.recordDetailCents(`${bareModel}-${fn}-output`, outputCents);

    this.recordDetailTokens(`${bareModel}-${fn}`, "input", usage.promptTokens);
    this.recordDetailTokens(`${bareModel}-${fn}`, "output", usage.completionTokens);
  }

  public recordTtsUsage(model: string, fn: string, text: string): void {
    const cents = (ttsCharacterCents[model] ?? 0) * text.length;
    this.ttsCents += cents;
    this.recordDetailCents(`${model}-${fn}`, cents);
    this.recordDetailChars(`${model}-${fn}`, text.length);
  }

  private recordDetailCents(key: string, cents: number): void {
    this.detailCents.set(key, this.detailCents.get(key) + cents);
  }

  private recordDetailTokens(
    key: string,
    type: "input" | "output",
    tokens: number,
  ): void {
    const existing = this.detailTokens.get(key);
    this.detailTokens.set(key, { ...existing, [type]: existing[type] + tokens });
  }

  private recordDetailChars(key: string, chars: number): void {
    this.detailChars.set(key, this.detailChars.get(key) + chars);
  }
}
