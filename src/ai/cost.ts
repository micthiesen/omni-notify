/**
 * Shared LLM token-cost pricing table + pure cost calculation, generalized
 * from `src/press-pods/costs.ts` (which additionally tracks TTS char costs
 * and per-episode accumulation — that stays PressPods-specific for now).
 * Reusable by anything that calls a model through `src/ai/registry.ts`:
 * email pipelines, briefings, recommendations, PressPods, etc.
 */

export interface TokenUsage {
  inputTokens: number;
  outputTokens: number;
}

export interface ModelPrice {
  /** USD cents per input (prompt) token. */
  inputCentsPerToken: number;
  /** USD cents per output (completion) token. */
  outputCentsPerToken: number;
}

// Prices are in USD cents per token, keyed by the bare model id (the part
// after "provider:" in a "provider:model" string, e.g. "google:gemini-3.5-flash"
// -> "gemini-3.5-flash"). Unlisted models cost 0 cents; use `hasPrice` to
// distinguish "genuinely free" from "no pricing data" so callers can warn.
// Sources: https://ai.google.dev/pricing (Gemini), OpenAI pricing page (GPT).
// Entries marked "estimated" are best-effort guesses (no confirmed public
// pricing found for this exact model id) — verify before trusting cost
// totals derived from them.
export const MODEL_PRICES: Record<string, ModelPrice> = {
  // --- Google Gemini --- (ported from src/press-pods/costs.ts)
  "gemini-3-flash-preview": {
    inputCentsPerToken: 0.00005, // $0.50 / 1M tokens
    outputCentsPerToken: 0.0003, // $3.00 / 1M tokens
  },
  "gemini-3.5-flash": {
    inputCentsPerToken: 0.00005, // $0.50 / 1M tokens
    outputCentsPerToken: 0.0003, // $3.00 / 1M tokens
  },
  "gemini-3.1-flash-lite": {
    inputCentsPerToken: 0.00001, // $0.10 / 1M tokens
    outputCentsPerToken: 0.00004, // $0.40 / 1M tokens
  },
  // ESTIMATED: no confirmed pricing found; assumed to sit between
  // gemini-3.1-flash-lite and gemini-3.5-flash, consistent with the "flash"
  // (non-lite, non-preview) tier of its generation.
  "gemini-3.1-flash": {
    inputCentsPerToken: 0.000015, // $0.15 / 1M tokens (estimated)
    outputCentsPerToken: 0.00006, // $0.60 / 1M tokens (estimated)
  },
  // ESTIMATED: no confirmed pricing found; "-lite" tiers have historically
  // held flat pricing across Gemini generations, so this mirrors
  // gemini-3.1-flash-lite.
  "gemini-3.5-flash-lite": {
    inputCentsPerToken: 0.00001, // $0.10 / 1M tokens (estimated)
    outputCentsPerToken: 0.00004, // $0.40 / 1M tokens (estimated)
  },

  // --- OpenAI ---
  // ESTIMATED: no confirmed pricing found for this model id; assumed
  // comparable to the GPT-4o-tier flagship rate.
  "gpt-5.6": {
    inputCentsPerToken: 0.00025, // $2.50 / 1M tokens (estimated)
    outputCentsPerToken: 0.001, // $10.00 / 1M tokens (estimated)
  },
  // ESTIMATED: no confirmed pricing found; "-luna" reads as the cheap/mini
  // variant (used for shortlist scoring / taste reflection where volume is
  // high), assumed comparable to the GPT-4o-mini-tier rate.
  "gpt-5.6-luna": {
    inputCentsPerToken: 0.000015, // $0.15 / 1M tokens (estimated)
    outputCentsPerToken: 0.00006, // $0.60 / 1M tokens (estimated)
  },
};

/** Strips a "provider:model" prefix, if present, down to the bare model id. */
export function bareModelId(modelId: string): string {
  return modelId.split(":").pop() ?? modelId;
}

/** True when `modelId` has a pricing entry (i.e. cost 0 means "unpriced", not "free"). */
export function hasPrice(modelId: string): boolean {
  return bareModelId(modelId) in MODEL_PRICES;
}

/**
 * Pure USD-cents cost for one LLM call. Returns 0 for unpriced models —
 * check `hasPrice` first if you want to warn rather than silently record $0.
 */
export function llmCostCents(modelId: string, usage: TokenUsage): number {
  const price = MODEL_PRICES[bareModelId(modelId)];
  if (!price) return 0;
  return (
    price.inputCentsPerToken * usage.inputTokens +
    price.outputCentsPerToken * usage.outputTokens
  );
}

/**
 * Minimal running-total accumulator for callers that make several LLM calls
 * per unit of work (e.g. one pipeline run) and want a single cost figure at
 * the end without re-deriving addition/lookup boilerplate each time.
 */
export class LlmCostCounter {
  private cents = 0;

  public add(modelId: string, usage: TokenUsage): number {
    const cost = llmCostCents(modelId, usage);
    this.cents += cost;
    return cost;
  }

  public getCents(): number {
    return this.cents;
  }
}
