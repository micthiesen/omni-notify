import type { Logger } from "@micthiesen/mitools/logging";
import type { FetchResult } from "../utils/fetchResult.js";
import { createCastroClient } from "./castro/client.js";

/**
 * Bridge to the user's podcast client account (Castro). Split read/write
 * surface modeled on the recommendations mediaLibrary/watchlist bridges.
 *
 * Castro has no public API. The implementation uses its captured private sync
 * protocol with the owner's device credentials. See docs/castro-sync.md for
 * the observed endpoints, authentication details, and remaining limitations.
 * Subscriptions and listen history are read from the account; when it is
 * unconfigured the pipeline degrades to the seed taste profile plus explicit
 * feedback. Recommendations are auto-enqueued when Castro resolves their RSS
 * identity, with the notification deep link retained as a fallback.
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

export interface PodcastSearchResult {
  /** Stable identifier assigned by the podcast client. */
  clientId: string;
  title: string;
  author?: string;
  feedUrl: string;
  itunesId?: number;
  summary?: string;
  artworkUrl?: string;
}

export interface PodcastEpisodeSearchResult {
  /** Stable identifier assigned by the podcast client. */
  clientId: string;
  title: string;
  showTitle: string;
  author?: string;
  publishedAt?: number;
  artworkUrl?: string;
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
  description?: string;
  /** Epoch ms the episode was queued, when known. */
  addedAt?: number;
}

export type PodcastWriteResult =
  | "added"
  | "removed"
  | "already_exists"
  | "not_found"
  | "unavailable"
  | "error";

export enum PodcastQueuePosition {
  Next = "next",
  Last = "last",
}

export interface EnqueueEpisodeRequest {
  feedUrl: string;
  itunesId?: number;
  /** RSS item GUID of the episode to queue. */
  episodeGuid: string;
  /**
   * Enclosure (audio) URL. Preferred episode key: hosting platforms rewrite
   * RSS guids so `episodeGuid` often will not match the client's stored guid,
   * but the enclosure URL is shared across both.
   */
  mediaUrl?: string;
  showTitle: string;
  episodeTitle: string;
  /** Where to place the episode in clients that support ordered insertion. */
  position?: PodcastQueuePosition;
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
   *
   * `sinceMs` (epoch) bounds how far back to look. Callers should pass the
   * tightest window they can — resolving every episode's metadata is the
   * heaviest thing this client does, so a narrow window keeps us a
   * well-behaved API consumer. Omitted means the implementation's default.
   */
  fetchListenHistory(sinceMs?: number): Promise<FetchResult<ListenedEpisode[]>>;
  fetchQueue(): Promise<FetchResult<QueuedEpisode[]>>;
  searchPodcasts(query: string): Promise<FetchResult<PodcastSearchResult[]>>;
  searchEpisodes(query: string): Promise<FetchResult<PodcastEpisodeSearchResult[]>>;

  /** Add one episode to the play queue. Idempotency: report already_exists. */
  enqueueEpisode(request: EnqueueEpisodeRequest): Promise<PodcastWriteResult>;
  /** Remove one episode from the play queue. Idempotency: report not_found. */
  dequeueEpisode(episodeGuid: string): Promise<PodcastWriteResult>;
  subscribeToShow(request: SubscribeToShowRequest): Promise<PodcastWriteResult>;
}

/** The configured podcast account client, or undefined when none is available. */
export function resolvePodcastAccount(
  logger: Logger,
): PodcastAccountClient | undefined {
  return createCastroClient(logger) ?? undefined;
}
