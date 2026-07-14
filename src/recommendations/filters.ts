import type { PooledCandidate } from "./candidates.js";

export interface FilterContext {
  /** Everything ever watched (any completion — partial watches shouldn't be re-pitched). */
  watchedIds: Set<string>;
  inProgressIds: Set<string>;
  watchlistIds: Set<string>;
  /** Recommended within cooldown, or terminally watched/abandoned. */
  excludedRecommendationIds: Set<string>;
}

export interface FilterOutcome {
  kept: PooledCandidate[];
  dropped: { canonicalId: string; title: string; reason: string }[];
}

/** Deterministic hard filters, applied before any model sees a candidate. */
export function filterEligible(
  pool: PooledCandidate[],
  context: FilterContext,
): FilterOutcome {
  const kept: PooledCandidate[] = [];
  const dropped: FilterOutcome["dropped"] = [];

  for (const candidate of pool) {
    const reason = dropReason(candidate.canonicalId, context);
    if (reason) {
      dropped.push({
        canonicalId: candidate.canonicalId,
        title: candidate.title,
        reason,
      });
    } else {
      kept.push(candidate);
    }
  }

  return { kept, dropped };
}

function dropReason(canonicalId: string, context: FilterContext): string | undefined {
  if (context.watchedIds.has(canonicalId)) return "already watched";
  if (context.inProgressIds.has(canonicalId)) return "currently in progress";
  if (context.watchlistIds.has(canonicalId)) return "already on watchlist";
  if (context.excludedRecommendationIds.has(canonicalId)) {
    return "recently recommended or terminal outcome";
  }
  return undefined;
}
