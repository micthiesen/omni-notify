import { randomUUID } from "node:crypto";
import type { Logger } from "@micthiesen/mitools/logging";
import { generateKeyBetween } from "fractional-indexing";
import { Cache, Clock, Effect, Ref } from "effect";
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
import { normalizeTitle } from "../titles.js";
import { normalizeFeedUrl } from "../types.js";
import { CastroApi, type CastroRequestError } from "./api.js";
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

// Action ids are process-wide Lamport timestamps. Multiple CastroClient
// instances can overlap (recommendations, cleanup, and MCP), so instance-local
// counters can collide when they emit actions in the same millisecond.
const actionIdRef = Effect.runSync(Ref.make(0));
// Queue placement is a read/decision/write transaction. Serialize it across
// every CastroClient instance so overlapping tasks cannot both observe the
// same queue and emit duplicate actions or the same fractional position.
const enqueueSemaphore = Effect.unsafeMakeSemaphore(1);

export class CastroClient implements PodcastAccountClient {
  public readonly name = "Castro";
  private readonly podcastCache: Cache.Cache<string, CastroPodcast, CastroRequestError>;
  private readonly episodeCache: Cache.Cache<string, CastroEpisode, CastroRequestError>;
  private readonly searchCache: Cache.Cache<
    string,
    CastroPodcastSearchResult[],
    CastroRequestError
  >;
  private readonly subscriptionsCache: Cache.Cache<
    "subscriptions",
    CastroProfileSubscription[],
    CastroRequestError
  >;

  private constructor(
    private readonly api: CastroApi,
    private readonly logger: Logger,
    caches: {
      podcast: Cache.Cache<string, CastroPodcast, CastroRequestError>;
      episode: Cache.Cache<string, CastroEpisode, CastroRequestError>;
      search: Cache.Cache<string, CastroPodcastSearchResult[], CastroRequestError>;
      subscriptions: Cache.Cache<
        "subscriptions",
        CastroProfileSubscription[],
        CastroRequestError
      >;
    },
  ) {
    this.podcastCache = caches.podcast;
    this.episodeCache = caches.episode;
    this.searchCache = caches.search;
    this.subscriptionsCache = caches.subscriptions;
  }

  public static make(api: CastroApi, logger: Logger): Effect.Effect<CastroClient> {
    return Effect.all({
      podcast: Cache.make({
        capacity: 500,
        timeToLive: "1 hour",
        lookup: (publicId: string) => api.fetchPodcast(publicId),
      }),
      episode: Cache.make({
        capacity: 2_000,
        timeToLive: "1 hour",
        lookup: (publicId: string) => api.fetchEpisode(publicId),
      }),
      search: Cache.make({
        capacity: 200,
        timeToLive: "1 hour",
        lookup: (query: string) => api.searchPodcasts(query),
      }),
      subscriptions: Cache.make({
        capacity: 1,
        timeToLive: "15 minutes",
        lookup: (_key: "subscriptions") => api.fetchSubscriptions(),
      }),
    }).pipe(Effect.map((caches) => new CastroClient(api, logger, caches)));
  }

  // Subscriptions are read by fetchSubscriptions, fetchListenHistory, and
  // subscribeToShow; memoize for the client's (per-run) lifetime so a single
  // run makes one GET /profile/subscriptions instead of several.
  private getSubscriptions(): Effect.Effect<CastroProfileSubscription[], unknown> {
    return this.subscriptionsCache.get("subscriptions");
  }

  public fetchSubscriptions(): Effect.Effect<FetchResult<PodcastSubscription[]>> {
    return Effect.gen(this, function* () {
      const subscriptions = yield* this.getSubscriptions();
      const podcasts = yield* Effect.forEach(
        subscriptions,
        ({ podcast_id }) => this.fetchPodcast(podcast_id),
        { concurrency: READ_CONCURRENCY },
      );
      const enriched = yield* Effect.forEach(
        podcasts,
        (podcast) =>
          Effect.gen(this, function* () {
            const match = yield* this.findPodcastSearchResult(
              podcast.title,
              (result) => result.tentacles_id === podcast.public_id,
            ).pipe(Effect.catchAll(() => Effect.succeed(undefined)));
            return {
              title: podcast.title,
              feedUrl: match?.feed_url,
              itunesId: match?.itunes_id,
            } satisfies PodcastSubscription;
          }),
        { concurrency: READ_CONCURRENCY },
      );
      return { status: "ok" as const, value: enriched };
    }).pipe(Effect.catchAll((error) => Effect.succeed(unavailable(error))));
  }

  public fetchListenHistory(
    sinceMs?: number,
  ): Effect.Effect<FetchResult<ListenedEpisode[]>> {
    return Effect.gen(this, function* () {
      const subscriptions = yield* this.getSubscriptions();
      const states = yield* Effect.forEach(
        subscriptions,
        (subscription) =>
          Effect.gen(this, function* () {
            const [podcast, state] = yield* Effect.all(
              [
                this.fetchPodcast(subscription.podcast_id),
                this.api.fetchPodcastState(subscription.podcast_id),
              ],
              { concurrency: 2 },
            );
            return { podcast, state };
          }),
        { concurrency: READ_CONCURRENCY },
      );
      // Never look back further than the caller asked; default to the full
      // window. Resolving each episode's metadata is the heaviest call this
      // client makes, so a tight cutoff keeps request volume low.
      const now = yield* Clock.currentTimeMillis;
      const cutoff = Math.max(
        now - HISTORY_WINDOW_MS,
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
      const history = yield* Effect.forEach(
        recent,
        ({ podcast, episodeState }) =>
          Effect.gen(this, function* () {
            const episode = yield* this.fetchEpisode(episodeState.episode_id);
            const completion = episodeState.is_played
              ? 1
              : episode.duration.seconds > 0
                ? Math.max(
                    0,
                    Math.min(
                      episodeState.progress_seconds / episode.duration.seconds,
                      1,
                    ),
                  )
                : undefined;
            return {
              showTitle: podcast.title,
              episodeTitle: episode.title,
              episodeGuid: episode.guid || episode.public_id,
              mediaUrl: episode.media_url,
              listenedAt: Date.parse(episodeState.last_played as string),
              completion,
              starred: episodeState.is_starred,
            } satisfies ListenedEpisode;
          }),
        { concurrency: READ_CONCURRENCY },
      );
      history.sort((a, b) => b.listenedAt - a.listenedAt);
      return { status: "ok" as const, value: history };
    }).pipe(Effect.catchAll((error) => Effect.succeed(unavailable(error))));
  }

  public fetchQueue(): Effect.Effect<FetchResult<QueuedEpisode[]>> {
    return Effect.gen(this, function* () {
      const { queue_items } = yield* this.api.fetchQueue();
      const ordered = [...queue_items].sort((a, b) =>
        compareFractionalPositions(a.fractional_position, b.fractional_position),
      );
      const queue = yield* Effect.forEach(
        ordered,
        (item) =>
          Effect.gen(this, function* () {
            const [podcast, episode] = yield* Effect.all(
              [this.fetchPodcast(item.podcast_id), this.fetchEpisode(item.episode_id)],
              { concurrency: 2 },
            );
            return {
              showTitle: podcast.title,
              episodeTitle: episode.title,
              episodeGuid: episode.guid || episode.public_id,
              description: episode.description,
            } satisfies QueuedEpisode;
          }),
        { concurrency: READ_CONCURRENCY },
      );
      return { status: "ok" as const, value: queue };
    }).pipe(Effect.catchAll((error) => Effect.succeed(unavailable(error))));
  }

  public fetchInbox(): Effect.Effect<FetchResult<InboxEpisode[]>> {
    return Effect.gen(this, function* () {
      const subscriptions = yield* this.getSubscriptions();
      const states = yield* Effect.forEach(
        subscriptions,
        (subscription) =>
          Effect.gen(this, function* () {
            return {
              podcastId: subscription.podcast_id,
              state: yield* this.api.fetchPodcastState(subscription.podcast_id),
            };
          }),
        { concurrency: READ_CONCURRENCY },
      );
      const newEpisodes = states.flatMap(({ podcastId, state }) =>
        state.episode_states
          .filter((episodeState) => episodeState.is_new)
          .map((episodeState) => ({ podcastId, episodeState })),
      );
      const inbox = yield* Effect.forEach(
        newEpisodes,
        ({ podcastId, episodeState }) =>
          Effect.gen(this, function* () {
            const [podcast, episode] = yield* Effect.all(
              [
                this.fetchPodcast(podcastId),
                this.fetchEpisode(episodeState.episode_id),
              ],
              { concurrency: 2 },
            );
            return {
              clientEpisodeId: episode.public_id,
              showTitle: podcast.title,
              episodeTitle: episode.title,
              episodeGuid: episode.guid || episode.public_id,
              description: episode.description,
            } satisfies InboxEpisode;
          }),
        { concurrency: READ_CONCURRENCY },
      );
      return { status: "ok" as const, value: inbox };
    }).pipe(Effect.catchAll((error) => Effect.succeed(unavailable(error))));
  }

  public searchPodcasts(
    query: string,
  ): Effect.Effect<FetchResult<PodcastSearchResult[]>> {
    return Effect.gen(this, function* () {
      const results = yield* this.searchPodcastMetadata(query);
      return {
        status: "ok" as const,
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
    }).pipe(Effect.catchAll((error) => Effect.succeed(unavailable(error))));
  }

  public searchEpisodes(
    query: string,
  ): Effect.Effect<FetchResult<PodcastEpisodeSearchResult[]>> {
    return Effect.gen(this, function* () {
      const results = yield* this.api.searchEpisodes(query);
      return {
        status: "ok" as const,
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
    }).pipe(Effect.catchAll((error) => Effect.succeed(unavailable(error))));
  }

  public enqueueEpisode(
    request: EnqueueEpisodeRequest,
  ): Effect.Effect<PodcastWriteResult> {
    const enqueue = Effect.gen(this, function* () {
      const resolved = yield* this.resolvePodcast(request);
      if (!resolved) return "not_found";
      const podcast = yield* this.fetchPodcast(resolved.tentacles_id);
      const episode = matchEpisode(podcast.episodes, request);
      if (!episode) return "not_found";

      const queue = yield* this.api.fetchQueue();
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
      const now = yield* Clock.currentTimeMillis;
      const actions = [
        yield* this.actionEffect(
          episode.public_id,
          CastroActionType.EpisodeQueued,
          now,
          {
            fractional_position: fractionalPosition,
          },
        ),
        yield* this.actionEffect(
          episode.public_id,
          CastroActionType.ClearEpisodeNew,
          now,
        ),
      ];
      yield* this.api.postActions(actions);
      return "added";
    });
    return enqueueSemaphore
      .withPermits(1)(enqueue)
      .pipe(
        Effect.catchAll((error) => {
          this.logger.error("Castro enqueue failed", (error as Error).message);
          return Effect.succeed("error" as const);
        }),
      );
  }

  public subscribeToShow(
    request: SubscribeToShowRequest,
  ): Effect.Effect<PodcastWriteResult> {
    return Effect.gen(this, function* () {
      const resolved = yield* this.resolvePodcast(request);
      if (!resolved) return "not_found";
      const subscriptions = yield* this.getSubscriptions();
      if (
        subscriptions.some(
          (subscription) => subscription.podcast_id === resolved.tentacles_id,
        )
      ) {
        return "already_exists";
      }
      const response = yield* this.api.subscribe([resolved.tentacles_id]);
      return response.subscribed.some(
        (subscription) => subscription.feed_id === resolved.tentacles_id,
      )
        ? "added"
        : "error";
    }).pipe(
      Effect.catchAll((error) => {
        this.logger.error("Castro subscribe lookup failed", (error as Error).message);
        return Effect.succeed("error" as const);
      }),
    );
  }

  public dequeueEpisode(episodeGuid: string): Effect.Effect<PodcastWriteResult> {
    return Effect.gen(this, function* () {
      const queue = yield* this.api.fetchQueue();
      const episodes = yield* Effect.forEach(
        queue.queue_items,
        (item) =>
          Effect.gen(this, function* () {
            return {
              item,
              episode: yield* this.fetchEpisode(item.episode_id),
            };
          }),
        { concurrency: READ_CONCURRENCY },
      );
      const match = episodes.find(
        ({ episode }) => (episode.guid || episode.public_id) === episodeGuid,
      );
      if (!match) return "not_found";
      const now = yield* Clock.currentTimeMillis;
      const actions = [
        yield* this.actionEffect(
          match.item.episode_id,
          CastroActionType.EpisodeDequeued,
          now,
        ),
        yield* this.actionEffect(
          match.item.episode_id,
          CastroActionType.ClearEpisodeNew,
          now,
        ),
      ];
      yield* this.api.postActions(actions);
      return "removed";
    }).pipe(
      Effect.catchAll((error) => {
        this.logger.error("Castro dequeue failed", (error as Error).message);
        return Effect.succeed("error" as const);
      }),
    );
  }

  public clearInboxEpisode(clientEpisodeId: string): Effect.Effect<PodcastWriteResult> {
    return Effect.gen(this, function* () {
      const now = yield* Clock.currentTimeMillis;
      const action = yield* this.actionEffect(
        clientEpisodeId,
        CastroActionType.ClearEpisodeNew,
        now,
      );
      yield* this.api.postActions([action]);
    }).pipe(
      Effect.as("removed" as const),
      Effect.catchAll((error) => {
        this.logger.error("Castro inbox clear failed", (error as Error).message);
        return Effect.succeed("error" as const);
      }),
    );
  }

  private fetchPodcast(publicId: string): Effect.Effect<CastroPodcast, unknown> {
    return this.podcastCache.get(publicId);
  }

  private fetchEpisode(publicId: string): Effect.Effect<CastroEpisode, unknown> {
    return this.episodeCache.get(publicId);
  }

  private searchPodcastMetadata(
    query: string,
  ): Effect.Effect<CastroPodcastSearchResult[], unknown> {
    return this.searchCache.get(query);
  }

  private findPodcastSearchResult(
    query: string,
    predicate: (result: CastroPodcastSearchResult) => boolean,
  ): Effect.Effect<CastroPodcastSearchResult | undefined, unknown> {
    return this.searchPodcastMetadata(query).pipe(
      Effect.map((results) => results.find(predicate)),
    );
  }

  private resolvePodcast(request: {
    feedUrl: string;
    itunesId?: number;
  }): Effect.Effect<CastroPodcastSearchResult | undefined, unknown> {
    const normalizedFeedUrl = normalizeFeedUrl(request.feedUrl);
    return this.findPodcastSearchResult(
      request.feedUrl,
      (result) =>
        normalizeFeedUrl(result.feed_url) === normalizedFeedUrl ||
        (request.itunesId !== undefined && result.itunes_id === request.itunesId),
    );
  }

  private actionEffect(
    episodeId: string,
    actionType: CastroActionType,
    timestamp: number,
    eventData?: object,
  ): Effect.Effect<CastroAction> {
    return Ref.modify(actionIdRef, (previous) => {
      const id = Math.max(previous + 1, timestamp);
      return [
        {
          id,
          episode_id: episodeId,
          origin_event_id: randomUUID(),
          origin_timestamp: timestamp,
          source: CastroActionSource.User,
          action_type: actionType,
          ...(eventData === undefined ? {} : { event_data: JSON.stringify(eventData) }),
        },
        id,
      ] as const;
    });
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

// One CastroApi (and thus one shared Effect rate-control runtime) for the whole process. The
// recommendation pipeline builds a fresh client per run and the cleanup task
// holds a long-lived one; sharing the underlying API means every request from
// either — including runs that overlap — funnels through a single pacing
// queue, so the per-second/concurrency caps are actually enforced device-wide.
// Only the HTTP layer is shared; each CastroClient keeps its own (per-run)
// metadata caches so a long-lived instance never serves a stale episode list.
let sharedApi: CastroApi | undefined;
let sharedApiCredentialKey: string | undefined;

export function sharedCastroApi(accessId: string, secret: string): CastroApi {
  const credentialKey = `${accessId}\0${secret}`;
  if (!sharedApi || sharedApiCredentialKey !== credentialKey) {
    sharedApi = new CastroApi({ accessId, secret: Buffer.from(secret, "utf8") });
    sharedApiCredentialKey = credentialKey;
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
  // Compatibility edge: task factories and MCP dependency assembly are
  // synchronous. Cache construction itself remains expressed as an Effect.
  return Effect.runSync(CastroClient.make(sharedCastroApi(accessId, secret), logger));
}
