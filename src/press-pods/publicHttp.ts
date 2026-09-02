import { type OptionsInit } from "got";
import { Effect } from "effect";
import {
  assertPublicHttpUrl as assertSharedPublicHttpUrl,
  assertPublicHttpUrlSyntax,
  createPublicDnsLookup,
  isPublicAddress,
  type LimitedTextResponse,
  publicGot,
  publicGotStream,
  readBufferResponseWithLimit,
  readTextResponseWithLimit,
} from "../effect/publicHttp.js";
import { PressPodsError, tryPromise } from "./effect.js";

export const PRESS_PODS_HTML_MAX_BYTES = 10 * 1024 * 1024;
export const PRESS_PODS_JSON_MAX_BYTES = 5 * 1024 * 1024;
export const PRESS_PODS_IMAGE_MAX_BYTES = 10 * 1024 * 1024;

export type PressPodsTextRequest = (
  url: string | URL,
  options: OptionsInit,
) => LimitedTextResponse;

export {
  assertPublicHttpUrlSyntax,
  createPublicDnsLookup,
  isPublicAddress,
  publicGot,
  publicGotStream,
};

export const assertPublicHttpUrl = (
  value: string | URL,
): Effect.Effect<URL, PressPodsError> =>
  assertSharedPublicHttpUrl(value).pipe(
    Effect.mapError(
      (cause) =>
        new PressPodsError({ operation: "validate public PressPods URL", cause }),
    ),
  );

export interface BoundedPublicBuffer {
  readonly body: Buffer;
  readonly headers: Record<string, string | string[] | undefined>;
}

export function fetchPublicBuffer(
  url: string,
  options: OptionsInit,
  operation: string,
  maxBytes: number,
  request: PressPodsTextRequest = publicGotStream,
): Effect.Effect<BoundedPublicBuffer, PressPodsError> {
  return tryPromise(operation, async (signal) => {
    const response = request(url, { ...options, signal });
    const body = await readBufferResponseWithLimit(response, maxBytes);
    return { body, headers: response.response?.headers ?? {} };
  });
}

export function fetchPublicText(
  url: string,
  options: OptionsInit,
  operation: string,
  maxBytes = PRESS_PODS_HTML_MAX_BYTES,
  request: PressPodsTextRequest = publicGotStream,
): Effect.Effect<string, PressPodsError> {
  return tryPromise(operation, (signal) =>
    readTextResponseWithLimit(request(url, { ...options, signal }), maxBytes),
  );
}

export function fetchPublicJson(
  url: string,
  options: OptionsInit,
  operation: string,
  maxBytes = PRESS_PODS_JSON_MAX_BYTES,
  request: PressPodsTextRequest = publicGotStream,
): Effect.Effect<unknown, PressPodsError> {
  return fetchPublicText(url, options, operation, maxBytes, request).pipe(
    Effect.flatMap((body) =>
      Effect.try({
        try: () => JSON.parse(body) as unknown,
        catch: (cause) =>
          new PressPodsError({ operation: `decode ${operation} JSON`, cause }),
      }),
    ),
  );
}

export function fetchPublicHtml(
  url: string,
  userAgent: string,
  request: PressPodsTextRequest = publicGotStream,
  maxBytes = PRESS_PODS_HTML_MAX_BYTES,
): Effect.Effect<string, PressPodsError> {
  return fetchPublicText(
    url,
    {
      headers: { "User-Agent": userAgent, Accept: "text/html" },
      timeout: { request: 20_000 },
      retry: { limit: 2, methods: ["GET"] },
    },
    "fetch public PressPods HTML",
    maxBytes,
    request,
  );
}
