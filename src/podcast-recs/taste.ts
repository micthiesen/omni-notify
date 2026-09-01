import { readFile } from "node:fs/promises";
import { Data, Effect } from "effect";
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
export function buildTasteDigest(
  subscriptions: SubscriptionState,
  seed: string,
): string {
  return [
    seed,
    formatSubscriptionsDigest(subscriptions),
    formatPodcastFeedbackDigest(),
    formatPodcastTasteProfileDigest(),
  ]
    .filter((section) => section.length > 0)
    .join("\n\n");
}

export class TasteSeedError extends Data.TaggedError("TasteSeedError")<{
  readonly path: string;
  readonly reason: "unreadable" | "malformed";
  readonly cause?: unknown;
}> {
  public override get message(): string {
    if (this.reason === "malformed") {
      return `Podcast taste seed at ${this.path} is empty`;
    }
    const detail = this.cause instanceof Error ? `: ${this.cause.message}` : "";
    return `Could not read podcast taste seed at ${this.path}${detail}`;
  }
}

export type TasteSeedReader = (path: string, signal: AbortSignal) => Promise<string>;

const readTasteSeed: TasteSeedReader = (path, signal) =>
  readFile(path, { encoding: "utf8", signal });

/** Load the configured seed without blocking Node, preserving I/O and interruption. */
export function loadTasteSeedEffect(
  path = config.PODCAST_TASTE_PATH,
  reader: TasteSeedReader = readTasteSeed,
): Effect.Effect<string, TasteSeedError> {
  if (!path) return Effect.succeed("");

  return Effect.tryPromise({
    try: (signal) => reader(path, signal),
    catch: (cause) => new TasteSeedError({ path, reason: "unreadable", cause }),
  }).pipe(
    Effect.map((contents) => contents.trim()),
    Effect.filterOrFail(
      (contents) => contents.length > 0,
      () => new TasteSeedError({ path, reason: "malformed" }),
    ),
  );
}
