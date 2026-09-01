import { Effect, Schema } from "effect";
import { readFetchResponseTextWithLimit } from "../../effect/publicHttp.js";
import { integrationEffect } from "../effect.js";

export const ARR_JSON_MAX_BYTES = 8 * 1024 * 1024;

export interface ArrConfig {
  url?: string;
  apiKey?: string;
  rootFolderPath?: string;
  qualityProfileId?: number;
}

export type HttpResult<T> =
  | { status: "ok"; value: T }
  | { status: "http_error"; statusCode: number }
  | { status: "unavailable" };

export type FetchImplementation = typeof fetch;

export type ArrConnectionConfig = Required<Pick<ArrConfig, "url" | "apiKey">>;

export function hasArrConnection(
  config: ArrConfig,
): config is ArrConfig & ArrConnectionConfig {
  return Boolean(config.url && config.apiKey);
}

export function isConfigured(config: ArrConfig): config is Required<ArrConfig> {
  return Boolean(
    config.url &&
    config.apiKey &&
    config.rootFolderPath &&
    Number.isInteger(config.qualityProfileId),
  );
}

export function requestJson<A, I>(
  config: ArrConnectionConfig,
  path: string,
  schema: Schema.Schema<A, I, never>,
  init: RequestInit = {},
  fetchImpl: FetchImplementation = fetch,
): Effect.Effect<HttpResult<A>> {
  const url = new URL(
    `api/v3/${path.replace(/^\//, "")}`,
    `${config.url.replace(/\/+$/, "")}/`,
  );

  const request = Effect.gen(function* () {
    const response = yield* integrationEffect(`Arr request ${path}`, (signal) =>
      fetchImpl(url, {
        ...init,
        headers: {
          Accept: "application/json",
          "X-Api-Key": config.apiKey,
          ...init.headers,
        },
        signal: init.signal ?? signal,
      }),
    );
    if (!response.ok) {
      return { status: "http_error" as const, statusCode: response.status };
    }
    const body = yield* integrationEffect(`read Arr response ${path}`, (signal) =>
      readFetchResponseTextWithLimit(response, ARR_JSON_MAX_BYTES, signal),
    );
    const raw = yield* integrationEffect(`parse Arr response ${path}`, () =>
      JSON.parse(body),
    );
    const value = yield* Schema.decodeUnknown(schema)(raw);
    return { status: "ok" as const, value };
  });
  return request.pipe(
    Effect.timeout("15 seconds"),
    Effect.catchAll(() => Effect.succeed({ status: "unavailable" as const })),
  );
}

export function postJson(value: unknown): RequestInit {
  return {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(value),
  };
}
