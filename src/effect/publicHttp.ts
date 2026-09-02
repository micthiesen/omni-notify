import { promises as dns, lookup } from "node:dns";
import { BlockList, isIP, type LookupFunction } from "node:net";
import { Data, Effect } from "effect";
import got, { type OptionsInit } from "got";

export const PUBLIC_HTTP_USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
export const PUBLIC_TEXT_MAX_BYTES = 10 * 1024 * 1024;

export interface LimitedTextResponse extends AsyncIterable<Uint8Array | string> {
  readonly response?: {
    readonly headers?: Record<string, string | string[] | undefined>;
  };
  readonly destroy?: (error?: Error) => void;
}

export type PublicTextRequest = (
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

export class PublicHttpError extends Data.TaggedError("PublicHttpError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `${this.operation}: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

function mappedIpv4(address: string): string | undefined {
  const dotted = /^::ffff:(\d{1,3}(?:\.\d{1,3}){3})$/i.exec(address);
  if (dotted) return dotted[1];
  const embedded = /^::(?:ffff:)?([\da-f]{1,4}):([\da-f]{1,4})$/i.exec(address);
  if (!embedded) return undefined;
  const value =
    Number.parseInt(embedded[1], 16) * 0x1_0000 + Number.parseInt(embedded[2], 16);
  return [value >>> 24, (value >>> 16) & 255, (value >>> 8) & 255, value & 255].join(
    ".",
  );
}

export function isPublicAddress(address: string): boolean {
  const mapped = mappedIpv4(address);
  if (mapped) return isPublicAddress(mapped);
  const family = isIP(address);
  if (family === 4) return !blockedAddresses.check(address, "ipv4");
  if (family === 6) return !blockedAddresses.check(address, "ipv6");
  return false;
}

export function assertPublicHttpUrlSyntax(value: string | URL): URL {
  const url = value instanceof URL ? value : new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("URLs must use HTTP or HTTPS");
  }
  if (url.username || url.password)
    throw new Error("URLs must not contain credentials");
  const hostname = url.hostname
    .toLowerCase()
    .replace(/\.$/, "")
    .replace(/^\[|\]$/g, "");
  if (!hostname || hostname === "localhost" || hostname.endsWith(".localhost")) {
    throw new Error("URLs must use a public host");
  }
  if (isIP(hostname) && !isPublicAddress(hostname)) {
    throw new Error("URLs must use a public host");
  }
  return url;
}

/** Validate both the URL syntax and its current DNS answers before work is
 * accepted. Connection-time DNS is still gated by `publicGot`, which closes
 * the rebinding and redirect window. */
export const assertPublicHttpUrl = Effect.fn("PublicHttp.assertUrl")(function* (
  value: string | URL,
) {
  const url = yield* Effect.try({
    try: () => assertPublicHttpUrlSyntax(value),
    catch: (cause) =>
      new PublicHttpError({ operation: "validate public HTTP URL", cause }),
  });
  const hostname = url.hostname.replace(/^\[|\]$/g, "");
  if (isIP(hostname)) return url;
  const addresses = yield* Effect.tryPromise({
    try: () => dns.lookup(hostname, { all: true, verbatim: true }),
    catch: (cause) =>
      new PublicHttpError({ operation: "resolve public HTTP URL", cause }),
  });
  if (
    addresses.length === 0 ||
    addresses.some(({ address }) => !isPublicAddress(address))
  ) {
    return yield* new PublicHttpError({
      operation: "validate public HTTP URL",
      cause: new Error("URL host must resolve only to public addresses"),
    });
  }
  return url;
});

export function createPublicDnsLookup(
  resolve: LookupFunction = lookup,
): LookupFunction {
  return (hostname, options, callback) => {
    const returnAll = options.all === true;
    resolve(
      hostname,
      { ...options, all: true, verbatim: true },
      (error, address, family) => {
        const addresses =
          typeof address === "string"
            ? [{ address, family: family ?? isIP(address) }]
            : (address ?? []);
        if (error) {
          callback(error, returnAll ? [] : "", family);
          return;
        }
        if (
          addresses.length === 0 ||
          addresses.some(({ address: resolved }) => !isPublicAddress(resolved))
        ) {
          callback(
            new Error("URL host must resolve only to public addresses"),
            returnAll ? [] : "",
            family,
          );
          return;
        }
        if (returnAll) {
          callback(null, addresses);
          return;
        }
        const first = addresses[0];
        callback(null, first.address, first.family);
      },
    );
  };
}

const publicDnsLookup = createPublicDnsLookup();

/** Got validates URL syntax before each request and resolves every connection,
 * including redirects, through a public-address-only DNS lookup. */
export function publicGot(url: string | URL, options: OptionsInit = {}) {
  const beforeRequest = options.hooks?.beforeRequest ?? [];
  const beforeRedirect = options.hooks?.beforeRedirect ?? [];
  return got(url, {
    ...options,
    dnsLookup: publicDnsLookup,
    hooks: {
      ...options.hooks,
      beforeRequest: [
        (requestOptions) => {
          if (requestOptions.url) assertPublicHttpUrlSyntax(requestOptions.url);
        },
        ...beforeRequest,
      ],
      beforeRedirect: [
        (requestOptions) => {
          if (requestOptions.url) assertPublicHttpUrlSyntax(requestOptions.url);
        },
        ...beforeRedirect,
      ],
    },
  });
}

/** Streaming form of publicGot, retaining the same DNS and redirect gates. */
export function publicGotStream(
  url: string | URL,
  options: OptionsInit = {},
): LimitedTextResponse {
  const beforeRequest = options.hooks?.beforeRequest ?? [];
  const beforeRedirect = options.hooks?.beforeRedirect ?? [];
  return got.stream(url, {
    ...options,
    dnsLookup: publicDnsLookup,
    hooks: {
      ...options.hooks,
      beforeRequest: [
        (requestOptions) => {
          if (requestOptions.url) assertPublicHttpUrlSyntax(requestOptions.url);
        },
        ...beforeRequest,
      ],
      beforeRedirect: [
        (requestOptions) => {
          if (requestOptions.url) assertPublicHttpUrlSyntax(requestOptions.url);
        },
        ...beforeRedirect,
      ],
    },
  });
}

/** Buffer a streamed response only while it remains within the strict byte limit. */
export async function readBufferResponseWithLimit(
  response: LimitedTextResponse,
  maxBytes: number,
): Promise<Buffer> {
  const chunks: Buffer[] = [];
  let receivedBytes = 0;

  const assertDeclaredLengthWithinLimit = () => {
    const contentLength = response.response?.headers?.["content-length"];
    const declaredBytes = Number(
      Array.isArray(contentLength) ? contentLength[0] : contentLength,
    );
    if (Number.isFinite(declaredBytes) && declaredBytes > maxBytes) {
      throw new Error(`Response exceeds the ${maxBytes}-byte limit`);
    }
  };

  const iterator = response[Symbol.asyncIterator]();
  let completed = false;
  let failed = false;
  let cleanupFailure: unknown;
  try {
    // Reject a declared oversized response before reading even an empty body.
    assertDeclaredLengthWithinLimit();
    while (true) {
      const result = await iterator.next();
      // The response metadata can become available while awaiting the first
      // read, including when the body has zero chunks.
      assertDeclaredLengthWithinLimit();
      if (result.done) {
        completed = true;
        break;
      }
      const buffer = Buffer.isBuffer(result.value)
        ? result.value
        : Buffer.from(result.value);
      receivedBytes += buffer.byteLength;
      if (receivedBytes > maxBytes) {
        throw new Error(`Response exceeds the ${maxBytes}-byte limit`);
      }
      chunks.push(buffer);
    }
  } catch (cause) {
    failed = true;
    response.destroy?.(cause instanceof Error ? cause : undefined);
    throw cause;
  } finally {
    if (!completed && iterator.return) {
      try {
        await iterator.return();
      } catch (cause) {
        if (!failed) cleanupFailure = cause;
      }
    }
  }
  if (cleanupFailure) throw cleanupFailure;
  return Buffer.concat(chunks, receivedBytes);
}

/** Buffer a text response only while it remains within the strict byte limit. */
export async function readTextResponseWithLimit(
  response: LimitedTextResponse,
  maxBytes: number,
): Promise<string> {
  return (await readBufferResponseWithLimit(response, maxBytes)).toString("utf8");
}

/** Read a Fetch response with fixed-length preflight, chunked overflow
 * detection, and explicit stream cancellation when the cap is exceeded. */
export async function readFetchResponseBufferWithLimit(
  response: Response,
  maxBytes: number,
  signal?: AbortSignal,
): Promise<Buffer> {
  const declaredBytes = Number(response.headers.get("content-length"));
  if (Number.isFinite(declaredBytes) && declaredBytes > maxBytes) {
    await response.body?.cancel("response too large");
    throw new Error(`Response exceeds the ${maxBytes}-byte limit`);
  }
  if (!response.body) return Buffer.alloc(0);

  const reader = response.body.getReader();
  const chunks: Buffer[] = [];
  let receivedBytes = 0;
  const onAbort = () => {
    // Cancellation is initiated from an event callback, so there is no caller
    // that can await its Promise. Observe a rejected cancellation explicitly;
    // the pending read still carries the interruption through the main path.
    void reader.cancel(signal?.reason).catch(() => undefined);
  };
  signal?.addEventListener("abort", onAbort, { once: true });
  try {
    while (true) {
      if (signal?.aborted) throw signal.reason;
      const { done, value } = await reader.read();
      // A cancelled Fetch reader may resolve a pending read as done even when
      // the underlying source rejects cancellation. Preserve interruption as
      // the observable result instead of returning a partial response.
      if (signal?.aborted) throw signal.reason;
      if (done) break;
      const chunk = Buffer.from(value);
      receivedBytes += chunk.byteLength;
      if (receivedBytes > maxBytes) {
        await reader.cancel("response too large");
        throw new Error(`Response exceeds the ${maxBytes}-byte limit`);
      }
      chunks.push(chunk);
    }
    return Buffer.concat(chunks, receivedBytes);
  } finally {
    signal?.removeEventListener("abort", onAbort);
    reader.releaseLock();
  }
}

export async function readFetchResponseTextWithLimit(
  response: Response,
  maxBytes: number,
  signal?: AbortSignal,
): Promise<string> {
  return (await readFetchResponseBufferWithLimit(response, maxBytes, signal)).toString(
    "utf8",
  );
}

export function fetchPublicText(
  url: string,
  options: OptionsInit,
  operation: string,
  request: PublicTextRequest = publicGotStream,
  maxBytes = PUBLIC_TEXT_MAX_BYTES,
): Effect.Effect<string, PublicHttpError> {
  return Effect.tryPromise({
    try: (signal) =>
      readTextResponseWithLimit(request(url, { ...options, signal }), maxBytes),
    catch: (cause) => new PublicHttpError({ operation, cause }),
  });
}
