import { DefaultMap } from "@micthiesen/mitools/collections";
import { Logger } from "@micthiesen/mitools/logging";
import { hasPrice, llmCostCents, TTS_CHARACTER_CENTS } from "../ai/cost.js";
import { currentCostFeature, recordCostEventSafely } from "../costs/persistence.js";

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
    if (!hasPrice(model)) {
      LOGGER.debug(`No pricing for model ${bareModel}; counting as $0`);
    }
    const inputCents = llmCostCents(model, {
      inputTokens: usage.promptTokens,
      outputTokens: 0,
    });
    const outputCents = llmCostCents(model, {
      inputTokens: 0,
      outputTokens: usage.completionTokens,
    });
    this.llmCents += inputCents + outputCents;

    this.recordDetailCents(`${bareModel}-${fn}-input`, inputCents);
    this.recordDetailCents(`${bareModel}-${fn}-output`, outputCents);

    this.recordDetailTokens(`${bareModel}-${fn}`, "input", usage.promptTokens);
    this.recordDetailTokens(`${bareModel}-${fn}`, "output", usage.completionTokens);
  }

  public recordTtsUsage(model: string, fn: string, text: string): void {
    const price = TTS_CHARACTER_CENTS[model];
    const cents = (price ?? 0) * text.length;
    this.ttsCents += cents;
    this.recordDetailCents(`${model}-${fn}`, cents);
    this.recordDetailChars(`${model}-${fn}`, text.length);
    recordCostEventSafely({
      category: "tts",
      feature: currentCostFeature("press-pods"),
      operation: fn,
      service: model === "eleven_v3" ? "elevenlabs" : "self-hosted",
      model,
      costCents: price === undefined ? null : cents,
      priceStatus: price === undefined ? "unknown" : price === 0 ? "free" : "estimated",
      usage: { characters: text.length, requests: 1 },
    });
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
