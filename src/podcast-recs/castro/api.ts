import got from "got";
import PQueue from "p-queue";
import type { z } from "zod";
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

/** Low-level client for the observed Castro Tentacles protocol. */
export class CastroApi {
  private readonly queue = new PQueue({
    concurrency: MAX_CONCURRENT_REQUESTS,
    interval: RATE_INTERVAL_MS,
    intervalCap: MAX_REQUESTS_PER_INTERVAL,
  });

  public constructor(private readonly credentials: CastroCredentials) {}

  public async getSyncStatus(): Promise<CastroSyncStatus> {
    return this.request("/profile/sync/status", {
      responseSchema: castroSyncStatusSchema,
    });
  }

  public async fetchEvents(since: number, limit = 1000): Promise<CastroEventsResponse> {
    return this.request(`/profile/events?since=${since}&limit=${limit}`, {
      responseSchema: castroEventsResponseSchema,
    });
  }

  public async fetchUserEvents(
    since: number,
    limit = 1000,
  ): Promise<CastroUserEventsResponse> {
    return this.request(`/profile/sync/user_events?since=${since}&limit=${limit}`, {
      responseSchema: castroUserEventsResponseSchema,
    });
  }

  public async fetchPodcast(publicId: string): Promise<CastroPodcast> {
    return this.request(`/podcasts/${encodeURIComponent(publicId)}`, {
      responseSchema: castroPodcastSchema,
    });
  }

  public async fetchEpisode(publicId: string): Promise<CastroEpisode> {
    return this.request(`/episodes/${encodeURIComponent(publicId)}`, {
      responseSchema: castroEpisodeSchema,
    });
  }

  public async searchPodcasts(
    searchTerm: string,
  ): Promise<CastroPodcastSearchResult[]> {
    return this.request(`/search?search_term=${encodeCastroQueryValue(searchTerm)}`, {
      responseSchema: castroPodcastSearchResultsSchema,
    });
  }

  public async searchEpisodes(
    searchTerm: string,
  ): Promise<CastroEpisodeSearchResult[]> {
    return this.request(
      `/episode_search?search_term=${encodeCastroQueryValue(searchTerm)}`,
      {
        responseSchema: castroEpisodeSearchResultsSchema,
      },
    );
  }

  public async fetchSubscriptions(): Promise<CastroProfileSubscription[]> {
    return this.request("/profile/subscriptions", {
      responseSchema: castroProfileSubscriptionsSchema,
    });
  }

  public async fetchQueue(): Promise<CastroQueue> {
    return this.request("/profile/sync/queue", {
      responseSchema: castroQueueSchema,
    });
  }

  public async fetchPodcastState(publicId: string): Promise<CastroPodcastState> {
    return this.request(
      `/profile/sync/podcast_state?podcast_id=${encodeURIComponent(publicId)}`,
      { responseSchema: castroPodcastStateSchema },
    );
  }

  public async postActions(actions: CastroAction[]): Promise<void> {
    await this.request("/profile/sync/actions", {
      method: "POST",
      body: { actions },
      emptyResponse: true,
    });
  }

  public async subscribe(feedIds: string[]): Promise<CastroSubscriptionResponse> {
    return this.request("/profile/subscriptions/subscribe", {
      method: "POST",
      body: { feed_ids: feedIds },
      responseSchema: castroSubscriptionResponseSchema,
    });
  }

  public async unsubscribe(feedIds: string[]): Promise<void> {
    await this.request("/profile/subscriptions/unsubscribe", {
      method: "POST",
      body: { feed_ids: feedIds },
      emptyResponse: true,
    });
  }

  private async request<T>(
    pathAndQuery: string,
    options: CastroRequestOptions<T>,
  ): Promise<T> {
    const method = options.method ?? "GET";
    const body = options.body === undefined ? "" : JSON.stringify(options.body);

    // Sign inside the queued task, not before it: the HMAC covers the Date
    // header, and under queue backlog the send can be seconds behind enqueue.
    // Computing the date at send time keeps the signature's Date honest.
    // POST retry is 0 — writes to /profile/sync/actions are non-idempotent at
    // the HTTP level; got already excludes POST from its default retry methods,
    // so this is belt-and-suspenders against a future default change.
    const response = await this.queue
      .add(() => {
        const date = new Date().toUTCString();
        const authHeaders = createCastroAuthHeaders(this.credentials, {
          method,
          pathAndQuery,
          date,
          body,
        });
        return got(`${CASTRO_ORIGIN}${pathAndQuery}`, {
          method,
          body: method === "POST" ? body : undefined,
          headers: {
            ...authHeaders,
            Accept: CASTRO_ACCEPT,
            "User-Agent": CASTRO_USER_AGENT,
            "X-Tentacles-App": "castro-ios",
            "X-Tentacles-Platform": "iOS",
          },
          retry: { limit: method === "GET" ? 2 : 0 },
          timeout: { request: 15_000 },
        });
      })
      .catch((error: unknown) => {
        // Surface the server's reason (e.g. "Sync is disabled" when the device
        // credential's sync session is off) — it otherwise hides inside got's
        // generic "status code 4xx" message and is painful to diagnose.
        const responseBody = (error as { response?: { body?: unknown } }).response
          ?.body;
        if (typeof responseBody === "string" && responseBody.length > 0) {
          throw new Error(
            `Castro ${method} ${pathAndQuery} failed: ${(error as Error).message} — ${responseBody.slice(0, 200)}`,
          );
        }
        throw error;
      });

    if (options.emptyResponse) return undefined as T;
    if (!options.responseSchema) {
      throw new Error(`No response schema configured for ${method} ${pathAndQuery}`);
    }
    return options.responseSchema.parse(JSON.parse(response.body));
  }
}
