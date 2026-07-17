import { readFileSync } from "node:fs";
import config from "../utils/config.js";
import { formatPodcastFeedbackDigest } from "./persistence.js";
import { formatPodcastTasteProfileDigest } from "./reflection/index.js";
import type { SubscriptionState } from "./subscriptions.js";
import { formatSubscriptionsDigest } from "./subscriptions.js";

/**
 * Combined taste evidence for model prompts, from four inputs:
 *
 * 1. A hand-written seed profile (markdown at PODCAST_TASTE_PATH — the same
 *    listener profile the old PodcastPicks briefing embedded in its prompt).
 * 2. Subscribed shows (the strongest implicit signal available today).
 * 3. Explicit good-pick/not-for-me feedback from the web UI.
 * 4. The versioned reflective taste profile distilled weekly from Castro
 *    listen history and recommendation outcomes (reflection/).
 */
export function buildTasteDigest(subscriptions: SubscriptionState): string {
  return [
    loadTasteSeed(),
    formatSubscriptionsDigest(subscriptions),
    formatPodcastFeedbackDigest(),
    formatPodcastTasteProfileDigest(),
  ]
    .filter((section) => section.length > 0)
    .join("\n\n");
}

export function loadTasteSeed(): string {
  const path = config.PODCAST_TASTE_PATH;
  if (!path) return "";
  return readFileSync(path, "utf8").trim();
}
