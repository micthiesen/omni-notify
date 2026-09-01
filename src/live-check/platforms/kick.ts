import type { Effect as EffectType } from "effect/Effect";
import { Data, Effect, Schema, SynchronizedRef } from "effect";
import type { OptionsInit } from "got";
import {
  type LimitedTextResponse,
  publicGotStream,
  readTextResponseWithLimit,
} from "../../effect/publicHttp.js";
import { type FetchedStatus, LiveStatus } from "./index.js";

const TIMEOUT_MS = 10_000;
const TOKEN_URL = "https://id.kick.com/oauth/token";
const CHANNELS_URL = "https://api.kick.com/public/v1/channels";
const TOKEN_REFRESH_LEEWAY_MS = 60_000;
export const KICK_TOKEN_MAX_BYTES = 128 * 1024;
export const KICK_CHANNELS_MAX_BYTES = 2 * 1024 * 1024;

export class KickApiError extends Data.TaggedError("KickApiError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation}: ${detail}`;
  }
}

const tokenResponseSchema = Schema.Struct({
  access_token: Schema.String,
  token_type: Schema.String,
  expires_in: Schema.Number,
});

type CachedToken = { readonly accessToken: string; readonly expiresAt: number };
// Serializing refreshes guarantees one client-credentials exchange for a burst
// of callers observing an expired token.
const tokenState = Effect.runSync(SynchronizedRef.make<CachedToken | null>(null));

interface KickStreamResponse extends LimitedTextResponse {
  readonly response?: {
    readonly headers?: Record<string, string | string[] | undefined>;
    readonly statusCode?: number;
  };
}

export type KickHttpRequest = (
  url: string | URL,
  options: OptionsInit,
) => KickStreamResponse;

interface KickDependencies {
  readonly request?: KickHttpRequest;
  readonly tokenMaxResponseBytes?: number;
  readonly channelsMaxResponseBytes?: number;
}

function requestKickJson<A>(
  url: string,
  options: OptionsInit,
  schema: Schema.Decoder<A>,
  operation: string,
  maxBytes: number,
  request: KickHttpRequest = publicGotStream,
): EffectType<
  { readonly statusCode: number; readonly body: string; readonly data?: A },
  KickApiError
> {
  return Effect.tryPromise({
    try: async (signal) => {
      const response = request(url, {
        ...options,
        signal,
        throwHttpErrors: false,
      });
      const body = await readTextResponseWithLimit(response, maxBytes);
      return { statusCode: response.response?.statusCode ?? 200, body };
    },
    catch: (cause) => new KickApiError({ operation, cause }),
  }).pipe(
    Effect.flatMap((response) => {
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return Effect.succeed(response);
      }
      const jsonSchema = Schema.fromJsonString(schema);
      return Schema.decodeUnknownEffect(jsonSchema)(response.body).pipe(
        Effect.map((data) => ({ ...response, data })),
        Effect.mapError(
          (cause) =>
            new KickApiError({ operation: `Decode ${operation.toLowerCase()}`, cause }),
        ),
      );
    }),
  );
}

function fetchAccessToken(
  dependencies: KickDependencies,
): EffectType<CachedToken, KickApiError> {
  const clientId = process.env.KICK_CLIENT_ID;
  const clientSecret = process.env.KICK_CLIENT_SECRET;
  if (!clientId || !clientSecret) {
    return Effect.fail(
      new KickApiError({
        operation: "Kick token configuration",
        cause: new Error(
          "KICK_CLIENT_ID and KICK_CLIENT_SECRET env vars are required for Kick",
        ),
      }),
    );
  }
  return requestKickJson(
    TOKEN_URL,
    {
      method: "POST",
      form: {
        grant_type: "client_credentials",
        client_id: clientId,
        client_secret: clientSecret,
      },
      timeout: { request: TIMEOUT_MS },
    },
    tokenResponseSchema,
    "Kick token request",
    dependencies.tokenMaxResponseBytes ?? KICK_TOKEN_MAX_BYTES,
    dependencies.request,
  ).pipe(
    Effect.flatMap((response) =>
      response.data
        ? Effect.succeed(response.data)
        : Effect.fail(
            new KickApiError({
              operation: "Kick token request",
              cause: new Error(
                `Kick token API returned ${response.statusCode}: ${response.body.slice(0, 200)}`,
              ),
            }),
          ),
    ),
    Effect.map((parsed) => ({
      accessToken: parsed.access_token,
      expiresAt: Date.now() + parsed.expires_in * 1_000 - TOKEN_REFRESH_LEEWAY_MS,
    })),
  );
}

function getAccessToken(
  staleToken: string | undefined,
  dependencies: KickDependencies,
): EffectType<string, KickApiError> {
  return SynchronizedRef.modifyEffect(tokenState, (cached) => {
    // On a 401, reuse a token that another caller already refreshed while this
    // caller was in flight. Only the caller still holding the stale generation
    // performs the exchange.
    if (cached && cached.expiresAt > Date.now() && cached.accessToken !== staleToken) {
      return Effect.succeed([cached.accessToken, cached] as const);
    }
    return fetchAccessToken(dependencies).pipe(
      Effect.map((fresh) => [fresh.accessToken, fresh] as const),
    );
  });
}

const categorySchema = Schema.NullOr(
  Schema.Struct({
    id: Schema.optional(Schema.Number),
    name: Schema.optional(Schema.String),
  }),
);
const streamSchema = Schema.NullOr(
  Schema.Struct({
    is_live: Schema.Boolean,
    viewer_count: Schema.optional(Schema.Number),
    start_time: Schema.optional(Schema.String),
  }),
);
const kickChannelSchema = Schema.Struct({
  slug: Schema.String,
  stream_title: Schema.String.pipe(
    Schema.withDecodingDefaultTypeKey(Effect.succeed("")),
  ),
  category: Schema.optional(categorySchema),
  stream: Schema.optional(streamSchema),
});
const kickChannelsResponseSchema = Schema.Struct({
  data: Schema.Array(kickChannelSchema),
  message: Schema.optional(Schema.String),
});
export type KickChannelsResponse = Schema.Schema.Type<
  typeof kickChannelsResponseSchema
>;

function requestChannels(
  username: string,
  bearer: string,
  dependencies: KickDependencies,
) {
  return requestKickJson(
    CHANNELS_URL,
    {
      searchParams: { slug: username },
      headers: { Authorization: `Bearer ${bearer}` },
      timeout: { request: TIMEOUT_MS },
    },
    kickChannelsResponseSchema,
    "Kick channels request",
    dependencies.channelsMaxResponseBytes ?? KICK_CHANNELS_MAX_BYTES,
    dependencies.request,
  );
}

export function fetchKickLiveStatus(
  {
    username,
  }: {
    username: string;
  },
  dependencies: KickDependencies = {},
): EffectType<FetchedStatus> {
  return Effect.gen(function* () {
    let token = yield* getAccessToken(undefined, dependencies);
    let response = yield* requestChannels(username, token, dependencies);
    if (response.statusCode === 401) {
      token = yield* getAccessToken(token, dependencies);
      response = yield* requestChannels(username, token, dependencies);
    }
    if (response.statusCode < 200 || response.statusCode >= 300) {
      return {
        status: LiveStatus.Unknown,
        error: `Kick API returned ${response.statusCode}: ${response.body.slice(0, 200)}`,
      } as FetchedStatus;
    }
    if (!response.data) {
      return yield* new KickApiError({
        operation: "Kick channels request",
        cause: new Error("Successful Kick response had no decoded body"),
      });
    }
    return extractLiveStatus(response.data);
  }).pipe(
    Effect.catch((error) =>
      Effect.succeed({
        status: LiveStatus.Unknown,
        error: error instanceof Error ? error.message : String(error),
      } as FetchedStatus),
    ),
  );
}

export function extractLiveStatus(data: KickChannelsResponse): FetchedStatus {
  const channel = data.data[0];
  if (!channel || !channel.stream?.is_live) return { status: LiveStatus.Offline };
  return {
    status: LiveStatus.Live,
    title: channel.stream_title || channel.slug,
    viewerCount: channel.stream.viewer_count,
    category: channel.category?.name,
  };
}

export function getKickLiveUrl(username: string): string {
  return `https://kick.com/${username}`;
}
