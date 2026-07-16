import { readFileSync } from "node:fs";
import type { Logger } from "@micthiesen/mitools/logging";
import config from "../utils/config.js";
import type { FetchResult } from "../utils/fetchResult.js";
import type { PodcastAccountClient, PodcastSubscription } from "./account.js";
import { normalizeTitle } from "./filters.js";
import { parseOpmlSubscriptions } from "./opml.js";
import { type CanonicalShowId, makeShowId } from "./types.js";

export interface SubscriptionState {
  subscriptions: PodcastSubscription[];
  showIds: Set<CanonicalShowId>;
  normalizedTitles: Set<string>;
  source: "account" | "opml" | "none";
}

/**
 * Subscribed shows, preferring the live podcast account (Castro, once the
 * bridge exists) and falling back to a static OPML export. Precedence:
 *
 * - account read ok → use it
 * - account unavailable/absent → OPML file when configured
 * - OPML configured but unreadable → unavailable (a configured source that
 *   fails must abort, never silently weaken exclusions)
 * - nothing configured → empty with a warning (subscribed-show exclusion is
 *   only as good as the data provided)
 */
export async function resolveSubscriptions(
  account: PodcastAccountClient | undefined,
  logger: Logger,
): Promise<FetchResult<SubscriptionState>> {
  if (account) {
    const result = await account.fetchSubscriptions();
    if (result.status === "ok") {
      return { status: "ok", value: buildState(result.value, "account") };
    }
    logger.warn(
      `${account.name} subscriptions unavailable (${result.reason}); trying OPML fallback`,
    );
  }

  const opmlPath = config.PODCAST_SUBSCRIPTIONS_PATH;
  if (opmlPath) {
    try {
      const subscriptions = parseOpmlSubscriptions(readFileSync(opmlPath, "utf8"));
      if (subscriptions.length === 0) {
        logger.warn(`No subscriptions parsed from OPML at ${opmlPath}`);
      }
      return { status: "ok", value: buildState(subscriptions, "opml") };
    } catch (error) {
      return {
        status: "unavailable",
        reason: `OPML read failed (${opmlPath}): ${(error as Error).message}`,
      };
    }
  }

  logger.warn(
    "No podcast subscription source configured; subscribed-show exclusion disabled",
  );
  return { status: "ok", value: buildState([], "none") };
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
    return "Subscribed shows: unknown (no subscription source configured).";
  }
  const titles = state.subscriptions.map((s) => s.title).sort();
  return `Shows the user already subscribes to (source: ${state.source}) — never recommend these, but they are strong taste evidence:\n${titles.map((t) => `- ${t}`).join("\n")}`;
}
