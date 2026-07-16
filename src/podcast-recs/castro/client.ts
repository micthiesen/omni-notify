import { randomUUID } from "node:crypto";
import type { Logger } from "@micthiesen/mitools/logging";
import { generateKeyBetween } from "fractional-indexing";
import config from "../../utils/config.js";
import { type FetchResult, unavailable } from "../../utils/fetchResult.js";
import {
  type EnqueueEpisodeRequest,
  type ListenedEpisode,
  type PodcastAccountClient,
  PodcastQueuePosition,
  type PodcastSubscription,
  type PodcastWriteResult,
  type QueuedEpisode,
  type SubscribeToShowRequest,
} from "../account.js";
import { CastroApi } from "./api.js";
import {
  type CastroAction,
  CastroActionSource,
  CastroActionType,
  type CastroEpisode,
  type CastroPodcast,
  type CastroProfileSubscription,
} from "./protocol.js";

const HISTORY_WINDOW_MS = 180 * 24 * 60 * 60 * 1000;
const READ_CONCURRENCY = 8;

class CastroClient implements PodcastAccountClient {
  public readonly name = "Castro";
  private readonly podcastCache = new Map<string, Promise<CastroPodcast>>();
  private readonly episodeCache = new Map<string, Promise<CastroEpisode>>();
  private nextActionId = Date.now();

  public constructor(
    private readonly api: CastroApi,
    private readonly logger: Logger,
  ) {}

  public async fetchSubscriptions(): Promise<FetchResult<PodcastSubscription[]>> {
    try {
      const subscriptions = await this.api.fetchSubscriptions();
      const podcasts = await mapConcurrent(
        subscriptions,
        READ_CONCURRENCY,
        ({ podcast_id }) => this.fetchPodcast(podcast_id),
      );
      return {
        status: "ok",
        value: podcasts.map((podcast) => ({ title: podcast.title })),
      };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async fetchListenHistory(): Promise<FetchResult<ListenedEpisode[]>> {
    try {
      const subscriptions = await this.api.fetchSubscriptions();
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
      const cutoff = Date.now() - HISTORY_WINDOW_MS;
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
          };
        },
      );
      return { status: "ok", value: queue };
    } catch (error) {
      return unavailable(error);
    }
  }

  public async enqueueEpisode(
    request: EnqueueEpisodeRequest,
  ): Promise<PodcastWriteResult> {
    try {
      const subscriptions = await this.api.fetchSubscriptions();
      const match = await this.findPodcastByTitle(subscriptions, request.showTitle);
      if (!match) return "not_found";
      const episode = match.episodes.find(
        (candidate) => candidate.guid === request.episodeGuid,
      );
      if (!episode) return "not_found";

      const queue = await this.api.fetchQueue();
      if (queue.queue_items.some((item) => item.episode_id === episode.public_id)) {
        return "already_exists";
      }
      const positions = queue.queue_items
        .map((item) => item.fractional_position)
        .sort(compareFractionalPositions);
      const position = request.position ?? PodcastQueuePosition.Next;
      const fractionalPosition =
        position === PodcastQueuePosition.Last
          ? generateKeyBetween(positions.at(-1) ?? null, null)
          : generateKeyBetween(null, positions[0] ?? null);
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
      const subscriptions = await this.api.fetchSubscriptions();
      if (await this.findPodcastByTitle(subscriptions, request.title)) {
        return "already_exists";
      }
      // The mutation accepts a Castro podcast UUID. No captured endpoint yet
      // resolves an arbitrary RSS URL to that UUID.
      return "unavailable";
    } catch (error) {
      this.logger.error("Castro subscribe lookup failed", (error as Error).message);
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

  private async findPodcastByTitle(
    subscriptions: CastroProfileSubscription[],
    title: string,
  ): Promise<CastroPodcast | undefined> {
    const normalized = normalizeTitle(title);
    const podcasts = await mapConcurrent(
      subscriptions,
      READ_CONCURRENCY,
      ({ podcast_id }) => this.fetchPodcast(podcast_id),
    );
    return podcasts.find((podcast) => normalizeTitle(podcast.title) === normalized);
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

function normalizeTitle(title: string): string {
  return title
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
}

function compareFractionalPositions(a: string, b: string): number {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
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

/** Returns the configured Castro client, or null when no credentials are set. */
export function createCastroClient(logger: Logger): PodcastAccountClient | null {
  const { CASTRO_ACCESS_ID: accessId, CASTRO_SECRET_KEY: secret } = config;
  if (!accessId && !secret) return null;
  if (!accessId || !secret) {
    logger.warn("Castro requires both CASTRO_ACCESS_ID and CASTRO_SECRET_KEY");
    return null;
  }
  return new CastroClient(
    new CastroApi({ accessId, secret: Buffer.from(secret, "utf8") }),
    logger,
  );
}
