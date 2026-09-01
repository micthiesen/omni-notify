import type { Effect as EffectType } from "effect/Effect";
import { Data, Effect, Schema } from "effect";
import {
  fetchPublicText,
  PUBLIC_HTTP_USER_AGENT,
  type PublicTextRequest,
} from "../../effect/publicHttp.js";

const TIMEOUT_MS = 10_000;
export const PLATFORM_GQL_MAX_BYTES = 2 * 1024 * 1024;
export const PLATFORM_HTML_MAX_BYTES = 10 * 1024 * 1024;

export interface GQLRequestOptions {
  url: string;
  clientId: string;
  query: string;
}

export class PlatformRequestError extends Data.TaggedError("PlatformRequestError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation}: ${detail}`;
  }
}

export function fetchGQL<A>(
  options: GQLRequestOptions,
  schema: Schema.Decoder<A>,
  dependencies: {
    readonly request?: PublicTextRequest;
    readonly maxResponseBytes?: number;
  } = {},
): EffectType<A, PlatformRequestError> {
  const { url, clientId, query } = options;
  const operation = `Failed to fetch GQL from ${url}`;
  return fetchPublicText(
    url,
    {
      method: "POST",
      json: { query },
      headers: {
        "Client-Id": clientId,
        "User-Agent": PUBLIC_HTTP_USER_AGENT,
      },
      timeout: { request: TIMEOUT_MS },
    },
    operation,
    dependencies.request,
    dependencies.maxResponseBytes ?? PLATFORM_GQL_MAX_BYTES,
  ).pipe(
    Effect.flatMap(Schema.decodeUnknownEffect(Schema.fromJsonString(schema))),
    Effect.mapError((cause) => new PlatformRequestError({ operation, cause })),
  );
}

export function fetchPageHtml(
  url: string,
  dependencies: {
    readonly request?: PublicTextRequest;
    readonly maxResponseBytes?: number;
  } = {},
): EffectType<string, PlatformRequestError> {
  const operation = `Failed to check live status for ${url}`;
  return fetchPublicText(
    url,
    {
      headers: { "User-Agent": PUBLIC_HTTP_USER_AGENT },
      timeout: { request: TIMEOUT_MS },
    },
    operation,
    dependencies.request,
    dependencies.maxResponseBytes ?? PLATFORM_HTML_MAX_BYTES,
  ).pipe(Effect.mapError((cause) => new PlatformRequestError({ operation, cause })));
}
