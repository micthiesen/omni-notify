import type { Logger } from "@micthiesen/mitools/logging";
import type { FetchResult } from "../utils/fetchResult.js";
import type { PodcastAccountClient, PodcastSubscription } from "./account.js";
import { normalizeTitle } from "./titles.js";
import { type CanonicalShowId, makeShowId } from "./types.js";

export interface SubscriptionState {
  subscriptions: PodcastSubscription[];
  showIds: Set<CanonicalShowId>;
  normalizedTitles: Set<string>;
  source: "account" | "none";
}

/**
 * Subscribed shows, read exclusively from the podcast account (Castro).
 * Subscriptions are both the exclusion list and the strongest taste signal,
 * so a configured account whose read fails must abort the run (three-state
 * rule) rather than silently proceed with weakened exclusions and risk
 * recommending an already-followed show.
 *
 * - account read ok → use it
 * - account read unavailable → abort the run
 * - no account configured → empty with a warning (the seed taste profile still
 *   drives prompt-level exclusion, but hard subscribed-show filtering is off)
 */
export async function resolveSubscriptions(
  account: PodcastAccountClient | undefined,
  logger: Logger,
): Promise<FetchResult<SubscriptionState>> {
  if (!account) {
    logger.warn(
      "No podcast account configured; subscribed-show exclusion falls back to the taste profile only",
    );
    return { status: "ok", value: buildState([], "none") };
  }

  const result = await account.fetchSubscriptions();
  if (result.status === "unavailable") {
    return {
      status: "unavailable",
      reason: `${account.name} subscriptions unavailable: ${result.reason}`,
    };
  }
  return { status: "ok", value: buildState(result.value, "account") };
}

function buildState(
  subscriptions: PodcastSubscription[],
  source: SubscriptionState["source"],
): SubscriptionState {
  const showIds = new Set<CanonicalShowId>();
  const normalizedTitles = new Set<string>();
  for (const sub of subscriptions) {
    const id = makeShowId(sub);
    if (id) showIds.add(id);
    normalizedTitles.add(normalizeTitle(sub.title));
  }
  return { subscriptions, showIds, normalizedTitles, source };
}

/** Compact digest of subscribed shows for model prompts. */
export function formatSubscriptionsDigest(state: SubscriptionState): string {
  if (state.subscriptions.length === 0) {
    return "Subscribed shows: unknown (no podcast account configured).";
  }
  const titles = state.subscriptions.map((s) => s.title).sort();
  return `Shows the user already subscribes to (source: ${state.source}) — never recommend these, but they are strong taste evidence:\n${titles.map((t) => `- ${t}`).join("\n")}`;
}
