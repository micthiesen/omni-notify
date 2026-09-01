import { promises as dns, lookup } from "node:dns";
import { BlockList, isIP, type LookupFunction } from "node:net";
import got, { type OptionsInit } from "got";
import { Effect } from "effect";
import {
  type LimitedTextResponse,
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

const blockedAddresses = new BlockList();

for (const [network, prefix] of [
  ["0.0.0.0", 8],
  ["10.0.0.0", 8],
  ["100.64.0.0", 10],
  ["127.0.0.0", 8],
  ["169.254.0.0", 16],
  ["172.16.0.0", 12],
  ["192.0.0.0", 24],
  ["192.0.2.0", 24],
  ["192.168.0.0", 16],
  ["198.18.0.0", 15],
  ["198.51.100.0", 24],
  ["203.0.113.0", 24],
  ["224.0.0.0", 4],
  ["240.0.0.0", 4],
] as const) {
  blockedAddresses.addSubnet(network, prefix, "ipv4");
}

for (const [network, prefix] of [
  ["::", 128],
  ["::1", 128],
  ["64:ff9b::", 96],
  ["64:ff9b:1::", 48],
  ["100::", 64],
  ["2001::", 32],
  ["2001:2::", 48],
  ["2001:10::", 28],
  ["2001:db8::", 32],
  ["2002::", 16],
  ["fc00::", 7],
  ["fe80::", 10],
  ["ff00::", 8],
] as const) {
  blockedAddresses.addSubnet(network, prefix, "ipv6");
}

function normalizedMappedIpv4(address: string): string | undefined {
  const match = /^::ffff:(\d{1,3}(?:\.\d{1,3}){3})$/i.exec(address);
  if (match) return match[1];

  const embedded = /^::(?:ffff:)?([\da-f]{1,4}):([\da-f]{1,4})$/i.exec(address);
  if (!embedded) return undefined;
  const value =
    Number.parseInt(embedded[1], 16) * 0x1_0000 + Number.parseInt(embedded[2], 16);
  return [value >>> 24, (value >>> 16) & 255, (value >>> 8) & 255, value & 255].join(
    ".",
  );
}

export function isPublicAddress(address: string): boolean {
  const mapped = normalizedMappedIpv4(address);
  if (mapped) return isPublicAddress(mapped);

  const family = isIP(address);
  if (family === 4) return !blockedAddresses.check(address, "ipv4");
  if (family === 6) return !blockedAddresses.check(address, "ipv6");
  return false;
}

export function assertPublicHttpUrlSyntax(value: string | URL): URL {
  const url = value instanceof URL ? value : new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("PressPods URLs must use HTTP or HTTPS");
  }
  if (url.username || url.password) {
    throw new Error("PressPods URLs must not contain credentials");
  }
  const hostname = url.hostname
    .toLowerCase()
    .replace(/\.$/, "")
    .replace(/^\[|\]$/g, "");
  if (!hostname || hostname === "localhost" || hostname.endsWith(".localhost")) {
    throw new Error("PressPods URLs must use a public host");
  }
  if (isIP(hostname) && !isPublicAddress(hostname)) {
    throw new Error("PressPods URLs must use a public host");
  }
  return url;
}

export const assertPublicHttpUrl = (
  value: string | URL,
): Effect.Effect<URL, PressPodsError> =>
  Effect.gen(function* () {
    const url = yield* Effect.try({
      try: () => assertPublicHttpUrlSyntax(value),
      catch: (cause) =>
        new PressPodsError({ operation: "validate public PressPods URL", cause }),
    });
    const hostname = url.hostname.replace(/^\[|\]$/g, "");
    if (isIP(hostname)) return url;
    const addresses = yield* tryPromise("resolve public PressPods URL", () =>
      dns.lookup(hostname, { all: true, verbatim: true }),
    );
    if (
      addresses.length === 0 ||
      addresses.some(({ address }) => !isPublicAddress(address))
    ) {
      return yield* new PressPodsError({
        operation: "validate public PressPods URL",
        cause: new Error("PressPods URLs must resolve only to public addresses"),
      });
    }
    return url;
  });

/** @deprecated Use the canonical Effect API, `assertPublicHttpUrl`. */
export const assertPublicHttpUrlEffect = assertPublicHttpUrl;

/**
 * Resolve every outbound connection through a public-address gate. Got invokes
 * this for each redirect, so a public URL cannot redirect into the LAN or a
 * loopback/link-local endpoint. IP-literal requests are rejected by the hook.
 */
export function createPublicDnsLookup(
  resolve: LookupFunction = lookup,
): LookupFunction {
  return (hostname, options, callback) => {
    const returnAll = options.all === true;
    resolve(
      hostname,
      { ...options, all: true, verbatim: true },
      (error, address, resultFamily) => {
        const resolvedAddresses =
          typeof address === "string"
            ? [{ address, family: resultFamily ?? isIP(address) }]
            : (address ?? []);
        if (error) {
          callback(error, returnAll ? [] : "", resultFamily);
          return;
        }
        if (
          resolvedAddresses.length === 0 ||
          resolvedAddresses.some(
            ({ address: resolvedAddress }) => !isPublicAddress(resolvedAddress),
          )
        ) {
          callback(
            new Error("PressPods URLs must resolve only to public addresses"),
            returnAll ? [] : "",
            resultFamily,
          );
          return;
        }
        if (returnAll) {
          callback(null, resolvedAddresses);
          return;
        }
        const first = resolvedAddresses[0];
        callback(null, first.address, first.family);
      },
    );
  };
}

const publicDnsLookup = createPublicDnsLookup();

export function publicGot(url: string | URL, options: OptionsInit = {}) {
  const existingBeforeRequest = options.hooks?.beforeRequest ?? [];
  const existingBeforeRedirect = options.hooks?.beforeRedirect ?? [];
  return got(url, {
    ...options,
    dnsLookup: publicDnsLookup,
    hooks: {
      ...options.hooks,
      beforeRequest: [
        async (requestOptions) => {
          if (requestOptions.url) {
            await Effect.runPromise(assertPublicHttpUrl(requestOptions.url));
          }
        },
        ...existingBeforeRequest,
      ],
      beforeRedirect: [
        (requestOptions) => {
          if (requestOptions.url) assertPublicHttpUrlSyntax(requestOptions.url);
        },
        ...existingBeforeRedirect,
      ],
    },
  });
}

export function publicGotStream(
  url: string | URL,
  options: OptionsInit = {},
): LimitedTextResponse {
  const existingBeforeRequest = options.hooks?.beforeRequest ?? [];
  const existingBeforeRedirect = options.hooks?.beforeRedirect ?? [];
  return got.stream(url, {
    ...options,
    dnsLookup: publicDnsLookup,
    hooks: {
      ...options.hooks,
      beforeRequest: [
        async (requestOptions) => {
          if (requestOptions.url) {
            await Effect.runPromise(assertPublicHttpUrl(requestOptions.url));
          }
        },
        ...existingBeforeRequest,
      ],
      beforeRedirect: [
        (requestOptions) => {
          if (requestOptions.url) assertPublicHttpUrlSyntax(requestOptions.url);
        },
        ...existingBeforeRedirect,
      ],
    },
  });
}

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
