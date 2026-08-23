import type { StreamerTier } from "./streamers.js";

export type LiveDisplayItem = {
  tier: StreamerTier;
  viewerCount: number | null;
  maxViewerCount: number;
  /** Rank-only override; the displayed viewer count remains `viewerCount`. */
  orderingViewerCount?: number;
};

export type OfflineDisplayItem = {
  lastEndedAt: number | null;
};

/**
 * One ordering primitive for every server-produced live list. Stable sort
 * preserves channels.json order when ranks tie, matching the dashboard's
 * existing behavior.
 */
export function compareLiveDisplayOrder(
  a: LiveDisplayItem,
  b: LiveDisplayItem,
): number {
  if (a.tier !== b.tier) return a.tier === "primary" ? -1 : 1;
  const aRank = a.orderingViewerCount ?? a.viewerCount ?? a.maxViewerCount;
  const bRank = b.orderingViewerCount ?? b.viewerCount ?? b.maxViewerCount;
  return bRank - aRank;
}

export function sortLiveDisplay<T extends LiveDisplayItem>(items: readonly T[]): T[] {
  return [...items].sort(compareLiveDisplayOrder);
}

export function sortOfflineDisplay<T extends OfflineDisplayItem>(
  items: readonly T[],
): T[] {
  return [...items].sort((a, b) => (b.lastEndedAt ?? 0) - (a.lastEndedAt ?? 0));
}
