/**
 * Canonical show identity: `itunes:{collectionId}` when Apple knows the show,
 * else `feed:{normalized feed URL}`. Show-level cooldowns, subscribed-show
 * exclusion, and not-for-me feedback all key on this.
 */
export type CanonicalShowId = string;

/** Canonical episode identity: `{showId}#{rss item guid}`. */
export type CanonicalEpisodeId = string;

export function makeShowId(args: {
  itunesId?: number;
  feedUrl?: string;
}): CanonicalShowId | undefined {
  if (args.itunesId) return `itunes:${args.itunesId}`;
  if (args.feedUrl) return `feed:${normalizeFeedUrl(args.feedUrl)}`;
  return undefined;
}

export function makeEpisodeId(
  showId: CanonicalShowId,
  episodeGuid: string,
): CanonicalEpisodeId {
  return `${showId}#${episodeGuid}`;
}

export function normalizeFeedUrl(feedUrl: string): string {
  return feedUrl
    .trim()
    .replace(/^https?:\/\//i, "")
    .replace(/\/+$/, "")
    .toLowerCase();
}

/** An episode surfaced by discovery, before resolution against iTunes/RSS. */
export interface DiscoveredEpisode {
  showTitle: string;
  episodeTitle: string;
  /** One-line note on why discovery surfaced it (thread title, list name...). */
  context: string;
  /** Where it was being discussed, when known. */
  sourceUrl?: string;
  /** Followed voices this episode features as guests, when guest-driven. */
  matchedVoices?: string[];
}

/** A fully resolved candidate: identity, verified release date, metadata. */
export interface EpisodeCandidate {
  episodeId: CanonicalEpisodeId;
  showId: CanonicalShowId;
  showTitle: string;
  episodeTitle: string;
  feedUrl: string;
  itunesId?: number;
  artworkUrl?: string;
  episodeGuid: string;
  /** Enclosure (audio) URL — the reliable key for matching against Castro. */
  mediaUrl?: string;
  /** Web page for the episode, when the feed provides one. */
  episodeUrl?: string;
  /** Verified from the show's RSS feed — never trusted from search snippets. */
  publishedAt: number;
  durationMinutes?: number;
  description: string;
  showGenres: string[];
  discoveredVia: string;
  sourceUrl?: string;
  /**
   * Followed voices (from the taste profile) appearing as guests on this
   * episode. Non-empty marks a Tier-1 "guest of someone I follow" candidate,
   * which the scorer boosts heavily and the pipeline near-auto-includes.
   */
  matchedVoices?: string[];
}
