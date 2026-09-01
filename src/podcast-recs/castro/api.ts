import { Data, Clock, Effect, Ref, Schedule } from "effect";
import type { z } from "zod";
import {
  type LimitedTextResponse,
  publicGotStream,
  readTextResponseWithLimit,
} from "../../effect/publicHttp.js";
import { type CastroCredentials, createCastroAuthHeaders } from "./auth.js";
import {
  type CastroAction,
  type CastroEpisode,
  type CastroEpisodeSearchResult,
  type CastroEventsResponse,
  type CastroPodcast,
  type CastroPodcastSearchResult,
  type CastroPodcastState,
  type CastroProfileSubscription,
  type CastroQueue,
  type CastroSubscriptionResponse,
  type CastroSyncStatus,
  type CastroUserEventsResponse,
  castroEpisodeSchema,
  castroEpisodeSearchResultsSchema,
  castroEventsResponseSchema,
  castroPodcastSchema,
  castroPodcastSearchResultsSchema,
  castroPodcastStateSchema,
  castroProfileSubscriptionsSchema,
  castroQueueSchema,
  castroSubscriptionResponseSchema,
  castroSyncStatusSchema,
  castroUserEventsResponseSchema,
} from "./protocol.js";

const CASTRO_ORIGIN = "https://tentacles.castro.fm";
const CASTRO_ACCEPT = "application/vnd.tentacles.supertop.co+json; version=8";
const CASTRO_USER_AGENT = "Castro/2396 CFNetwork/3890.100.1 Darwin/27.0.0";
export const CASTRO_RESPONSE_MAX_BYTES = 5 * 1024 * 1024;

// Global pacing so we stay a well-behaved client regardless of which method
// fans out. Every request (including nested per-episode fetches) funnels
// through one queue, capping both simultaneous connections and requests per
// second. Values sit comfortably under what a power user's app would burst.
const MAX_CONCURRENT_REQUESTS = 4;
const MAX_REQUESTS_PER_INTERVAL = 8;
const RATE_INTERVAL_MS = 1000;

export function encodeCastroQueryValue(value: string): string {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

interface CastroRequestOptions<T> {
  method?: "GET" | "POST";
  body?: unknown;
  responseSchema?: z.ZodType<T>;
  emptyResponse?: boolean;
}

interface CastroStreamResponse extends LimitedTextResponse {
  readonly response?: {
    readonly headers?: Record<string, string | string[] | undefined>;
    readonly statusCode?: number;
  };
}

export type CastroHttpRequest = (
  url: string | URL,
  options: Parameters<typeof publicGotStream>[1],
) => CastroStreamResponse;

export class CastroRequestError extends Data.TaggedError("CastroRequestError")<{
  readonly method: "GET" | "POST";
  readonly pathAndQuery: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `Castro ${this.method} ${this.pathAndQuery} failed: ${detail}`;
  }
}

interface RequestControl {
  readonly apply: <A, E, R>(effect: Effect.Effect<A, E, R>) => Effect.Effect<A, E, R>;
}

const requestControl = Effect.runSync(
  Effect.gen(function* () {
    const semaphore = yield* Effect.makeSemaphore(MAX_CONCURRENT_REQUESTS);
    const rateState = yield* Ref.make({ windowStart: 0, used: 0 });
    const takeRatePermit: Effect.Effect<void> = Effect.suspend(() =>
      Effect.flatMap(Clock.currentTimeMillis, (now) =>
        Ref.modify(rateState, (state) => {
          if (now - state.windowStart >= RATE_INTERVAL_MS) {
            return [0, { windowStart: now, used: 1 }] as const;
          }
          if (state.used < MAX_REQUESTS_PER_INTERVAL) {
            return [0, { ...state, used: state.used + 1 }] as const;
          }
          return [state.windowStart + RATE_INTERVAL_MS - now, state] as const;
        }),
      ).pipe(
        Effect.flatMap((waitMs) =>
          waitMs > 0
            ? Effect.sleep(`${waitMs} millis`).pipe(Effect.zipRight(takeRatePermit))
            : Effect.void,
        ),
      ),
    );
    return {
      apply: (effect) =>
        takeRatePermit.pipe(Effect.zipRight(semaphore.withPermits(1)(effect))),
    } satisfies RequestControl;
  }),
);

/** Low-level client for the observed Castro Tentacles protocol. */
export class CastroApi {
  public constructor(
    private readonly credentials: CastroCredentials,
    private readonly dependencies: {
      readonly request?: CastroHttpRequest;
      readonly maxResponseBytes?: number;
    } = {},
  ) {}

  public getSyncStatus(): Effect.Effect<CastroSyncStatus, CastroRequestError> {
    return this.request("/profile/sync/status", {
      responseSchema: castroSyncStatusSchema,
    });
  }

  public fetchEvents(
    since: number,
    limit = 1000,
  ): Effect.Effect<CastroEventsResponse, CastroRequestError> {
    return this.request(`/profile/events?since=${since}&limit=${limit}`, {
      responseSchema: castroEventsResponseSchema,
    });
  }

  public fetchUserEvents(
    since: number,
    limit = 1000,
  ): Effect.Effect<CastroUserEventsResponse, CastroRequestError> {
    return this.request(`/profile/sync/user_events?since=${since}&limit=${limit}`, {
      responseSchema: castroUserEventsResponseSchema,
    });
  }

  public fetchPodcast(
    publicId: string,
  ): Effect.Effect<CastroPodcast, CastroRequestError> {
    return this.request(`/podcasts/${encodeURIComponent(publicId)}`, {
      responseSchema: castroPodcastSchema,
    });
  }

  public fetchEpisode(
    publicId: string,
  ): Effect.Effect<CastroEpisode, CastroRequestError> {
    return this.request(`/episodes/${encodeURIComponent(publicId)}`, {
      responseSchema: castroEpisodeSchema,
    });
  }

  public searchPodcasts(
    searchTerm: string,
  ): Effect.Effect<CastroPodcastSearchResult[], CastroRequestError> {
    return this.request(`/search?search_term=${encodeCastroQueryValue(searchTerm)}`, {
      responseSchema: castroPodcastSearchResultsSchema,
    });
  }

  public searchEpisodes(
    searchTerm: string,
  ): Effect.Effect<CastroEpisodeSearchResult[], CastroRequestError> {
    return this.request(
      `/episode_search?search_term=${encodeCastroQueryValue(searchTerm)}`,
      {
        responseSchema: castroEpisodeSearchResultsSchema,
      },
    );
  }

  public fetchSubscriptions(): Effect.Effect<
    CastroProfileSubscription[],
    CastroRequestError
  > {
    return this.request("/profile/subscriptions", {
      responseSchema: castroProfileSubscriptionsSchema,
    });
  }

  public fetchQueue(): Effect.Effect<CastroQueue, CastroRequestError> {
    return this.request("/profile/sync/queue", {
      responseSchema: castroQueueSchema,
    });
  }

  public fetchPodcastState(
    publicId: string,
  ): Effect.Effect<CastroPodcastState, CastroRequestError> {
    return this.request(
      `/profile/sync/podcast_state?podcast_id=${encodeURIComponent(publicId)}`,
      { responseSchema: castroPodcastStateSchema },
    );
  }

  public postActions(actions: CastroAction[]): Effect.Effect<void, CastroRequestError> {
    return this.request("/profile/sync/actions", {
      method: "POST",
      body: { actions },
      emptyResponse: true,
    });
  }

  public subscribe(
    feedIds: string[],
  ): Effect.Effect<CastroSubscriptionResponse, CastroRequestError> {
    return this.request("/profile/subscriptions/subscribe", {
      method: "POST",
      body: { feed_ids: feedIds },
      responseSchema: castroSubscriptionResponseSchema,
    });
  }

  public unsubscribe(feedIds: string[]): Effect.Effect<void, CastroRequestError> {
    return this.request("/profile/subscriptions/unsubscribe", {
      method: "POST",
      body: { feed_ids: feedIds },
      emptyResponse: true,
    });
  }

  private request<T>(
    pathAndQuery: string,
    options: CastroRequestOptions<T>,
  ): Effect.Effect<T, CastroRequestError> {
    const method = options.method ?? "GET";
    const body = options.body === undefined ? "" : JSON.stringify(options.body);

    // Sign inside the queued task, not before it: the HMAC covers the Date
    // header, and under queue backlog the send can be seconds behind enqueue.
    // Computing the date at send time keeps the signature's Date honest.
    // POST retry is 0 — writes to /profile/sync/actions are non-idempotent at
    // the HTTP level; got already excludes POST from its default retry methods,
    // so this is belt-and-suspenders against a future default change.
    const attempt = requestControl.apply(
      Effect.tryPromise({
        try: async (signal) => {
          const date = new Date().toUTCString();
          const authHeaders = createCastroAuthHeaders(this.credentials, {
            method,
            pathAndQuery,
            date,
            body,
          });
          const response = (this.dependencies.request ?? publicGotStream)(
            `${CASTRO_ORIGIN}${pathAndQuery}`,
            {
              method,
              body: method === "POST" ? body : undefined,
              headers: {
                ...authHeaders,
                Accept: CASTRO_ACCEPT,
                "User-Agent": CASTRO_USER_AGENT,
                "X-Tentacles-App": "castro-ios",
                "X-Tentacles-Platform": "iOS",
              },
              retry: { limit: 0 },
              signal,
              timeout: { request: 15_000 },
              throwHttpErrors: false,
            },
          ) as CastroStreamResponse;
          const responseBody = await readTextResponseWithLimit(
            response,
            this.dependencies.maxResponseBytes ?? CASTRO_RESPONSE_MAX_BYTES,
          );
          const statusCode = response.response?.statusCode ?? 200;
          if (statusCode < 200 || statusCode >= 300) {
            throw new Error(
              `Castro ${method} ${pathAndQuery} failed with HTTP ${statusCode}${responseBody.length > 0 ? `: ${responseBody.slice(0, 200)}` : ""}`,
            );
          }
          return responseBody;
        },
        catch: (cause) => new CastroRequestError({ method, pathAndQuery, cause }),
      }),
    );
    // Retry the rate-limited attempt, so every network request consumes its
    // own token and permit. Writes stay single-attempt because they are not
    // HTTP-idempotent.
    const request =
      method === "GET"
        ? attempt.pipe(
            Effect.retry(
              Schedule.exponential("200 millis").pipe(
                Schedule.compose(Schedule.recurs(2)),
              ),
            ),
          )
        : attempt;
    return request.pipe(
      Effect.flatMap((responseBody) => {
        if (options.emptyResponse) return Effect.succeed(undefined as T);
        if (!options.responseSchema) {
          return Effect.fail(
            new CastroRequestError({
              method,
              pathAndQuery,
              cause: new Error(
                `No response schema configured for ${method} ${pathAndQuery}`,
              ),
            }),
          );
        }
        return Effect.try({
          try: () => options.responseSchema!.parse(JSON.parse(responseBody)),
          catch: (cause) => new CastroRequestError({ method, pathAndQuery, cause }),
        });
      }),
    );
  }
}
