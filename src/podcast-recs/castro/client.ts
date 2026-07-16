import { randomUUID } from "node:crypto";
import type { Logger } from "@micthiesen/mitools/logging";
import { generateKeyBetween } from "fractional-indexing";
import config from "../../utils/config.js";
import { type FetchResult, unavailable } from "../../utils/fetchResult.js";
import {
  type EnqueueEpisodeRequest,
  type InboxEpisode,
  type ListenedEpisode,
  type PodcastAccountClient,
  type PodcastEpisodeSearchResult,
  PodcastQueuePosition,
  type PodcastSearchResult,
  type PodcastSubscription,
  type PodcastWriteResult,
  type QueuedEpisode,
  type SubscribeToShowRequest,
} from "../account.js";
import { normalizeTitle } from "../rss.js";
import { normalizeFeedUrl } from "../types.js";
import { CastroApi } from "./api.js";
import {
  type CastroAction,
  CastroActionSource,
  CastroActionType,
  type CastroEpisode,
  type CastroPodcast,
  type CastroPodcastSearchResult,
  type CastroProfileSubscription,
} from "./protocol.js";

const HISTORY_WINDOW_MS = 180 * 24 * 60 * 60 * 1000;
const READ_CONCURRENCY = 8;

export class CastroClient implements PodcastAccountClient {
  public readonly name = "Castro";
  private readonly podcastCache = new Map<string, Promise<CastroPodcast>>();
  private readonly episodeCache = new Map<string, Promise<CastroEpisode>>();
  private readonly searchCache = new Map<
    string,
    Promise<CastroPodcastSearchResult[]>
  >();
  private subscriptionsCache?: Promise<CastroProfileSubscription[]>;
  private nextActionId = Date.now();

  public constructor(
    private readonly api: CastroApi,
    private readonly logger: Logger,
  ) {}

  // Subscriptions are read by fetchSubscriptions, fetchListenHistory, and
  // subscribeToShow; memoize for the client's (per-run) lifetime so a single
  // run makes one GET /profile/subscriptions instead of several.
  private getSubscriptions(): Promise<CastroProfileSubscription[]> {
    if (this.subscriptionsCache) return this.subscriptionsCache;
    const pending = this.api.fetchSubscriptions().catch((error) => {
      this.subscriptionsCache = undefined;
      throw error;
    });
    this.subscriptionsCache = pending;
    return pending;
  }

  public async fetchSubscriptions(): Promise<FetchResult<PodcastSubscription[]>> {
    try {
      const subscriptions = await this.getSubscriptions();
      const podcasts = await mapConcurrent(
        subscriptions,
        READ_CONCURRENCY,
        ({ podcast_id }) => this.fetchPodcast(podcast_id),
      );
      const enriched = await mapConcurrent(
        podcasts,
        READ_CONCURRENCY,
        async (podcast): Promise<PodcastSubscription> => {
          const match = await this.findPodcastSearchResult(
            podcast.title,
            (result) => result.tentacles_id === podcast.public_id,
          ).catch(() => undefined);
          return {
            title: podcast.title,
            feedUrl: match?.feed_url,
            itunesId: match?.itunes_id,
          };
        },
      );
      return { status: "ok", value: enriched };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async fetchListenHistory(
    sinceMs?: number,
  ): Promise<FetchResult<ListenedEpisode[]>> {
    try {
      const subscriptions = await this.getSubscriptions();
      const states = await mapConcurrent(
        subscriptions,
        READ_CONCURRENCY,
        async (subscription) => {
          const [podcast, state] = await Promise.all([
            this.fetchPodcast(subscription.podcast_id),
            this.api.fetchPodcastState(subscription.podcast_id),
          ]);
          return { podcast, state };
        },
      );
      // Never look back further than the caller asked; default to the full
      // window. Resolving each episode's metadata is the heaviest call this
      // client makes, so a tight cutoff keeps request volume low.
      const cutoff = Math.max(
        Date.now() - HISTORY_WINDOW_MS,
        sinceMs ?? Number.NEGATIVE_INFINITY,
      );
      const recent = states.flatMap(({ podcast, state }) =>
        state.episode_states
          .filter(
            (episodeState) =>
              episodeState.last_played !== null &&
              Date.parse(episodeState.last_played) >= cutoff,
          )
          .map((episodeState) => ({ podcast, episodeState })),
      );
      const history = await mapConcurrent(
        recent,
        READ_CONCURRENCY,
        async ({ podcast, episodeState }): Promise<ListenedEpisode> => {
          const episode = await this.fetchEpisode(episodeState.episode_id);
          const completion = episodeState.is_played
            ? 1
            : episode.duration.seconds > 0
              ? Math.max(
                  0,
                  Math.min(episodeState.progress_seconds / episode.duration.seconds, 1),
                )
              : undefined;
          return {
            showTitle: podcast.title,
            episodeTitle: episode.title,
            episodeGuid: episode.guid || episode.public_id,
            listenedAt: Date.parse(episodeState.last_played as string),
            completion,
            starred: episodeState.is_starred,
          };
        },
      );
      history.sort((a, b) => b.listenedAt - a.listenedAt);
      return { status: "ok", value: history };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async fetchQueue(): Promise<FetchResult<QueuedEpisode[]>> {
    try {
      const { queue_items } = await this.api.fetchQueue();
      const ordered = [...queue_items].sort((a, b) =>
        compareFractionalPositions(a.fractional_position, b.fractional_position),
      );
      const queue = await mapConcurrent(
        ordered,
        READ_CONCURRENCY,
        async (item): Promise<QueuedEpisode> => {
          const [podcast, episode] = await Promise.all([
            this.fetchPodcast(item.podcast_id),
            this.fetchEpisode(item.episode_id),
          ]);
          return {
            showTitle: podcast.title,
            episodeTitle: episode.title,
            episodeGuid: episode.guid || episode.public_id,
            description: episode.description,
          };
        },
      );
      return { status: "ok", value: queue };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async fetchInbox(): Promise<FetchResult<InboxEpisode[]>> {
    try {
      const subscriptions = await this.getSubscriptions();
      const states = await mapConcurrent(
        subscriptions,
        READ_CONCURRENCY,
        async (subscription) => ({
          podcastId: subscription.podcast_id,
          state: await this.api.fetchPodcastState(subscription.podcast_id),
        }),
      );
      const newEpisodes = states.flatMap(({ podcastId, state }) =>
        state.episode_states
          .filter((episodeState) => episodeState.is_new)
          .map((episodeState) => ({ podcastId, episodeState })),
      );
      const inbox = await mapConcurrent(
        newEpisodes,
        READ_CONCURRENCY,
        async ({ podcastId, episodeState }): Promise<InboxEpisode> => {
          const [podcast, episode] = await Promise.all([
            this.fetchPodcast(podcastId),
            this.fetchEpisode(episodeState.episode_id),
          ]);
          return {
            clientEpisodeId: episode.public_id,
            showTitle: podcast.title,
            episodeTitle: episode.title,
            episodeGuid: episode.guid || episode.public_id,
            description: episode.description,
          };
        },
      );
      return { status: "ok", value: inbox };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async searchPodcasts(
    query: string,
  ): Promise<FetchResult<PodcastSearchResult[]>> {
    try {
      const results = await this.searchPodcastMetadata(query);
      return {
        status: "ok",
        value: results.map((result) => ({
          clientId: result.tentacles_id,
          title: result.title,
          author: result.author ?? undefined,
          feedUrl: result.feed_url,
          itunesId: result.itunes_id,
          summary: result.summary ?? undefined,
          artworkUrl: result.artwork_url.large,
        })),
      };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async searchEpisodes(
    query: string,
  ): Promise<FetchResult<PodcastEpisodeSearchResult[]>> {
    try {
      const results = await this.api.searchEpisodes(query);
      return {
        status: "ok",
        value: results.map((result) => {
          const publishedAt = Date.parse(result.published_at);
          return {
            clientId: result.tentacles_id,
            title: result.title,
            showTitle: result.podcast_name,
            author: result.author ?? undefined,
            publishedAt: Number.isFinite(publishedAt) ? publishedAt : undefined,
            artworkUrl: result.artwork_url ?? result.podcast_artwork_url ?? undefined,
          };
        }),
      };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async enqueueEpisode(
    request: EnqueueEpisodeRequest,
  ): Promise<PodcastWriteResult> {
    try {
      const resolved = await this.resolvePodcast(request);
      if (!resolved) return "not_found";
      const podcast = await this.fetchPodcast(resolved.tentacles_id);
      const episode = matchEpisode(podcast.episodes, request);
      if (!episode) return "not_found";

      const queue = await this.api.fetchQueue();
      if (queue.queue_items.some((item) => item.episode_id === episode.public_id)) {
        return "already_exists";
      }
      const positions = queue.queue_items
        .map((item) => item.fractional_position)
        .sort(compareFractionalPositions);
      const position = request.position ?? PodcastQueuePosition.Next;
      // "Queue Next" matches the app: insert AFTER the current top item (which
      // is playing / up next), i.e. as the new 2nd item — not above it. On an
      // empty or single-item queue this naturally lands first or second.
      const fractionalPosition =
        position === PodcastQueuePosition.Last
          ? generateKeyBetween(positions.at(-1) ?? null, null)
          : generateKeyBetween(positions[0] ?? null, positions[1] ?? null);
      const now = Date.now();
      await this.api.postActions([
        this.action(episode.public_id, CastroActionType.EpisodeQueued, now, {
          fractional_position: fractionalPosition,
        }),
        this.action(episode.public_id, CastroActionType.ClearEpisodeNew, now),
      ]);
      return "added";
    } catch (error) {
      this.logger.error("Castro enqueue failed", (error as Error).message);
      return "error";
    }
  }

  public async subscribeToShow(
    request: SubscribeToShowRequest,
  ): Promise<PodcastWriteResult> {
    try {
      const resolved = await this.resolvePodcast(request);
      if (!resolved) return "not_found";
      const subscriptions = await this.getSubscriptions();
      if (
        subscriptions.some(
          (subscription) => subscription.podcast_id === resolved.tentacles_id,
        )
      ) {
        return "already_exists";
      }
      const response = await this.api.subscribe([resolved.tentacles_id]);
      return response.subscribed.some(
        (subscription) => subscription.feed_id === resolved.tentacles_id,
      )
        ? "added"
        : "error";
    } catch (error) {
      this.logger.error("Castro subscribe lookup failed", (error as Error).message);
      return "error";
    }
  }

  public async dequeueEpisode(episodeGuid: string): Promise<PodcastWriteResult> {
    try {
      const queue = await this.api.fetchQueue();
      const episodes = await mapConcurrent(
        queue.queue_items,
        READ_CONCURRENCY,
        async (item) => ({
          item,
          episode: await this.fetchEpisode(item.episode_id),
        }),
      );
      const match = episodes.find(
        ({ episode }) => (episode.guid || episode.public_id) === episodeGuid,
      );
      if (!match) return "not_found";
      const now = Date.now();
      await this.api.postActions([
        this.action(match.item.episode_id, CastroActionType.EpisodeDequeued, now),
        this.action(match.item.episode_id, CastroActionType.ClearEpisodeNew, now),
      ]);
      return "removed";
    } catch (error) {
      this.logger.error("Castro dequeue failed", (error as Error).message);
      return "error";
    }
  }

  public async clearInboxEpisode(clientEpisodeId: string): Promise<PodcastWriteResult> {
    try {
      await this.api.postActions([
        this.action(clientEpisodeId, CastroActionType.ClearEpisodeNew, Date.now()),
      ]);
      return "removed";
    } catch (error) {
      this.logger.error("Castro inbox clear failed", (error as Error).message);
      return "error";
    }
  }

  private fetchPodcast(publicId: string): Promise<CastroPodcast> {
    const cached = this.podcastCache.get(publicId);
    if (cached) return cached;
    const pending = this.api.fetchPodcast(publicId).catch((error) => {
      this.podcastCache.delete(publicId);
      throw error;
    });
    this.podcastCache.set(publicId, pending);
    return pending;
  }

  private fetchEpisode(publicId: string): Promise<CastroEpisode> {
    const cached = this.episodeCache.get(publicId);
    if (cached) return cached;
    const pending = this.api.fetchEpisode(publicId).catch((error) => {
      this.episodeCache.delete(publicId);
      throw error;
    });
    this.episodeCache.set(publicId, pending);
    return pending;
  }

  private searchPodcastMetadata(query: string): Promise<CastroPodcastSearchResult[]> {
    const cacheKey = query.trim().toLocaleLowerCase();
    const cached = this.searchCache.get(cacheKey);
    if (cached) return cached;
    const pending = this.api.searchPodcasts(query).catch((error) => {
      this.searchCache.delete(cacheKey);
      throw error;
    });
    this.searchCache.set(cacheKey, pending);
    return pending;
  }

  private async findPodcastSearchResult(
    query: string,
    predicate: (result: CastroPodcastSearchResult) => boolean,
  ): Promise<CastroPodcastSearchResult | undefined> {
    const results = await this.searchPodcastMetadata(query);
    return results.find(predicate);
  }

  private async resolvePodcast(request: {
    feedUrl: string;
    itunesId?: number;
  }): Promise<CastroPodcastSearchResult | undefined> {
    const normalizedFeedUrl = normalizeFeedUrl(request.feedUrl);
    return this.findPodcastSearchResult(
      request.feedUrl,
      (result) =>
        normalizeFeedUrl(result.feed_url) === normalizedFeedUrl ||
        (request.itunesId !== undefined && result.itunes_id === request.itunesId),
    );
  }

  private action(
    episodeId: string,
    actionType: CastroActionType,
    timestamp: number,
    eventData?: object,
  ): CastroAction {
    return {
      id: this.nextActionId++,
      episode_id: episodeId,
      origin_event_id: randomUUID(),
      origin_timestamp: timestamp,
      source: CastroActionSource.User,
      action_type: actionType,
      ...(eventData === undefined ? {} : { event_data: JSON.stringify(eventData) }),
    };
  }
}

function compareFractionalPositions(a: string, b: string): number {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

/**
 * Match a requested episode against a Castro podcast's episodes. RSS `<guid>`
 * is unreliable — hosting platforms (Simplecast, Megaphone) rewrite it, so
 * Castro's stored `guid` (equal to its own `public_id`) frequently differs
 * from the feed's guid. The enclosure/media URL is shared across both and is
 * the strongest key; a unique title match is the last resort.
 */
export function matchEpisode(
  episodes: CastroEpisode[],
  request: EnqueueEpisodeRequest,
): CastroEpisode | undefined {
  const byGuid = episodes.find((episode) => episode.guid === request.episodeGuid);
  if (byGuid) return byGuid;

  const mediaKey = normalizeMediaUrl(request.mediaUrl);
  if (mediaKey) {
    const byMedia = episodes.find(
      (episode) => normalizeMediaUrl(episode.media_url) === mediaKey,
    );
    if (byMedia) return byMedia;
  }

  // Title is ambiguous when a show reuses episode titles, so only accept it
  // when exactly one episode matches.
  const titleKey = normalizeTitle(request.episodeTitle);
  if (titleKey) {
    const byTitle = episodes.filter(
      (episode) => normalizeTitle(episode.title) === titleKey,
    );
    if (byTitle.length === 1) return byTitle[0];
  }
  return undefined;
}

/** Compare enclosure URLs by host+path, ignoring protocol and query params. */
export function normalizeMediaUrl(url: string | undefined): string | undefined {
  if (!url) return undefined;
  const trimmed = url.trim().toLowerCase();
  if (!trimmed) return undefined;
  const noProtocol = trimmed.replace(/^https?:\/\//, "");
  const queryIndex = noProtocol.indexOf("?");
  return queryIndex === -1 ? noProtocol : noProtocol.slice(0, queryIndex);
}

async function mapConcurrent<T, R>(
  values: T[],
  concurrency: number,
  mapper: (value: T) => Promise<R>,
): Promise<R[]> {
  const results = new Array<R>(values.length);
  let cursor = 0;
  await Promise.all(
    Array.from({ length: Math.min(concurrency, values.length) }, async () => {
      while (cursor < values.length) {
        const index = cursor++;
        results[index] = await mapper(values[index] as T);
      }
    }),
  );
  return results;
}

// One CastroApi (and thus one rate-limit queue) for the whole process. The
// recommendation pipeline builds a fresh client per run and the cleanup task
// holds a long-lived one; sharing the underlying API means every request from
// either — including runs that overlap — funnels through a single pacing
// queue, so the per-second/concurrency caps are actually enforced device-wide.
// Only the HTTP layer is shared; each CastroClient keeps its own (per-run)
// metadata caches so a long-lived instance never serves a stale episode list.
let sharedApi: CastroApi | undefined;

function sharedCastroApi(accessId: string, secret: string): CastroApi {
  if (!sharedApi) {
    sharedApi = new CastroApi({ accessId, secret: Buffer.from(secret, "utf8") });
  }
  return sharedApi;
}

/** Returns the configured Castro client, or null when no credentials are set. */
export function createCastroClient(logger: Logger): PodcastAccountClient | null {
  const { CASTRO_ACCESS_ID: accessId, CASTRO_SECRET_KEY: secret } = config;
  if (!accessId && !secret) return null;
  if (!accessId || !secret) {
    logger.warn("Castro requires both CASTRO_ACCESS_ID and CASTRO_SECRET_KEY");
    return null;
  }
  return new CastroClient(sharedCastroApi(accessId, secret), logger);
}
