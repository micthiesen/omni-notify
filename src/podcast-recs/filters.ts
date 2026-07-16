import type { PodcastExclusions } from "./persistence.js";
import type { CanonicalShowId, EpisodeCandidate } from "./types.js";

/** Only episodes released within this window are recommendable. */
export const RECENT_EPISODE_WINDOW_MS = 7 * 24 * 60 * 60 * 1000;

export interface PodcastFilterContext {
  now: number;
  /** Canonical ids of shows the user already subscribes to. */
  subscribedShowIds: Set<CanonicalShowId>;
  /** Normalized titles of subscribed shows (fallback when ids are missing). */
  subscribedShowTitles: Set<string>;
  exclusions: PodcastExclusions;
}

export interface DroppedEpisode {
  candidate: EpisodeCandidate;
  reason: string;
}

export interface PodcastFilterResult {
  kept: EpisodeCandidate[];
  dropped: DroppedEpisode[];
}

/**
 * Pure hard filters applied before any model call. The whole point of this
 * feature is surfacing shows the user does NOT already follow, so subscribed
 * shows are excluded outright.
 */
export function filterEligibleEpisodes(
  candidates: EpisodeCandidate[],
  context: PodcastFilterContext,
): PodcastFilterResult {
  const kept: EpisodeCandidate[] = [];
  const dropped: DroppedEpisode[] = [];

  for (const candidate of candidates) {
    const reason = disqualify(candidate, context);
    if (reason) dropped.push({ candidate, reason });
    else kept.push(candidate);
  }

  return { kept, dropped };
}

function disqualify(
  candidate: EpisodeCandidate,
  context: PodcastFilterContext,
): string | undefined {
  const age = context.now - candidate.publishedAt;
  if (age > RECENT_EPISODE_WINDOW_MS) {
    const days = Math.floor(age / (24 * 60 * 60 * 1000));
    return `released ${days}d ago (outside recency window)`;
  }
  if (candidate.publishedAt > context.now + 24 * 60 * 60 * 1000) {
    return "release date in the future (feed metadata suspect)";
  }
  if (context.exclusions.episodeIds.has(candidate.episodeId)) {
    return "episode already recommended";
  }
  if (context.exclusions.showIds.has(candidate.showId)) {
    return "show on cooldown or excluded by feedback";
  }
  if (context.subscribedShowIds.has(candidate.showId)) {
    return "already subscribed";
  }
  if (context.subscribedShowTitles.has(normalizeTitle(candidate.showTitle))) {
    return "already subscribed (title match)";
  }
  return undefined;
}

/** Shared loose title normalization for cross-system show matching. */
export function normalizeTitle(title: string): string {
  return title
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/['’]/g, "")
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}
