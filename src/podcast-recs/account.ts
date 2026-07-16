import type { Logger } from "@micthiesen/mitools/logging";
import type { FetchResult } from "../utils/fetchResult.js";
import { createCastroClient } from "./castro/client.js";

/**
 * Bridge to the user's podcast client account (Castro). Split read/write
 * surface modeled on the recommendations mediaLibrary/watchlist bridges.
 *
 * Implementation status: Castro has no public API — see docs/castro-sync.md
 * for everything known about its sync architecture and the investigation
 * plan. Until a real client lands, resolvePodcastAccount() returns undefined
 * and the podcast-recs pipeline degrades gracefully:
 *   - subscriptions fall back to an OPML export (PODCAST_SUBSCRIPTIONS_PATH)
 *   - listen-history-driven outcome labeling is skipped entirely (explicit
 *     feedback from the web UI still works)
 *   - "acquisition" is a deep link in the notification instead of enqueueEpisode
 *
 * Contract notes for implementers:
 *   - Reads return FetchResult: `unavailable` MUST be distinguishable from an
 *     empty list. The pipeline aborts decisions that would be wrong against
 *     missing state rather than treating it as empty (project invariant).
 *   - Castro's device sync is an event-driven CRDT replicated via Castro's
 *     servers, so these snapshot-shaped reads may be backed by a locally
 *     maintained replica rather than a single REST GET. That is an
 *     implementation detail; callers only see snapshots.
 *   - Identify shows by feedUrl and/or itunesId whenever possible. Titles are
 *     display data and a last-resort matching key.
 */

/** A show the account is subscribed to. */
export interface PodcastSubscription {
  title: string;
  /** RSS feed URL — the strongest cross-system identity; provide if at all possible. */
  feedUrl?: string;
  /** Apple Podcasts collection id, when known. */
  itunesId?: number;
}

/** A playback event/state for one episode, newest state wins. */
export interface ListenedEpisode {
  showTitle: string;
  episodeTitle: string;
  /** RSS item GUID when known, else a stable client-native id. */
  episodeGuid?: string;
  feedUrl?: string;
  itunesId?: number;
  /** Epoch ms of the most recent playback activity. */
  listenedAt: number;
  /** 0-1 fraction of the episode listened, when the client reports it. */
  completion?: number;
  /** Explicit positive signal (starred/favorited), when the client supports it. */
  starred?: boolean;
}

/** An episode currently in the play queue (or inbox, for Castro). */
export interface QueuedEpisode {
  showTitle: string;
  episodeTitle: string;
  episodeGuid?: string;
  feedUrl?: string;
  /** Epoch ms the episode was queued, when known. */
  addedAt?: number;
}

export type PodcastWriteResult =
  | "added"
  | "already_exists"
  | "not_found"
  | "unavailable"
  | "error";

export interface EnqueueEpisodeRequest {
  feedUrl: string;
  /** RSS item GUID of the episode to queue. */
  episodeGuid: string;
  showTitle: string;
  episodeTitle: string;
}

export interface SubscribeToShowRequest {
  title: string;
  feedUrl: string;
  itunesId?: number;
}

export interface PodcastAccountClient {
  /** Human-readable client name for logs (e.g. "Castro"). */
  readonly name: string;

  fetchSubscriptions(): Promise<FetchResult<PodcastSubscription[]>>;
  /**
   * Playback history, newest first. Completion fractions power outcome
   * labeling (listened ≥80% vs abandoned), so include them when available.
   */
  fetchListenHistory(): Promise<FetchResult<ListenedEpisode[]>>;
  fetchQueue(): Promise<FetchResult<QueuedEpisode[]>>;

  /** Add one episode to the play queue. Idempotency: report already_exists. */
  enqueueEpisode(request: EnqueueEpisodeRequest): Promise<PodcastWriteResult>;
  subscribeToShow(request: SubscribeToShowRequest): Promise<PodcastWriteResult>;
}

/** The configured podcast account client, or undefined when none is available. */
export function resolvePodcastAccount(
  logger: Logger,
): PodcastAccountClient | undefined {
  return createCastroClient(logger) ?? undefined;
}
