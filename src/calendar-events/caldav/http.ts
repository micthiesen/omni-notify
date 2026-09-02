import { Duration, Effect, Schema } from "effect";
import { CaldavError } from "../effect.js";

const MAX_REDIRECTS = 5;
export const CALDAV_REQUEST_TIMEOUT_MS = 15_000;
export const CALDAV_XML_MAX_BYTES = 2 * 1024 * 1024;
export const CALDAV_ERROR_MAX_BYTES = 64 * 1024;
const CaldavXmlSchema = Schema.String.pipe(
  Schema.check(
    Schema.makeFilter((value) => value.trimStart().startsWith("<"), {
      message: "CalDAV response is not XML",
    }),
  ),
);

class CaldavResponseTooLargeError extends Error {}

/**
 * Run one bounded CalDAV request. Effect interruption and the request timeout
 * both abort the underlying fetch, including requests whose side effect may
 * otherwise continue after the caller has stopped waiting.
 */
export function requestCaldavEffect(
  input: string,
  init: RequestInit,
  operation: string,
): Effect.Effect<Response, CaldavError> {
  return Effect.tryPromise({
    try: (signal) =>
      fetch(input, {
        ...init,
        signal,
      }),
    catch: (cause) => new CaldavError({ operation, cause, transient: true }),
  }).pipe(
    Effect.timeout(Duration.millis(CALDAV_REQUEST_TIMEOUT_MS)),
    Effect.mapError((cause) =>
      cause instanceof CaldavError
        ? cause
        : new CaldavError({ operation, cause, transient: true }),
    ),
  );
}

function isIcloudCaldavHost(hostname: string): boolean {
  return (
    hostname === "caldav.icloud.com" || /^p\d+-caldav\.icloud\.com$/i.test(hostname)
  );
}

/** Reject URLs that could disclose a provider password to another origin. */
export function assertTrustedCaldavUrl(
  url: string,
  provider: "icloud" | "fastmail",
): string {
  const parsed = new URL(url);
  const trusted =
    parsed.protocol === "https:" &&
    (provider === "icloud"
      ? isIcloudCaldavHost(parsed.hostname)
      : parsed.hostname === "caldav.fastmail.com");
  if (!trusted) {
    throw new Error(`Untrusted ${provider} CalDAV URL: ${parsed.origin}`);
  }
  return parsed.toString();
}

function isSafeRedirect(from: URL, to: URL): boolean {
  if (to.protocol !== "https:") return false;
  if (from.hostname === to.hostname) return true;
  return isIcloudCaldavHost(from.hostname) && isIcloudCaldavHost(to.hostname);
}

/** Read a CalDAV response without allowing a server to exhaust process memory. */
export function readCaldavResponseText(
  response: Response,
  operation: string,
  maxBytes: number,
): Effect.Effect<string, CaldavError> {
  return Effect.tryPromise({
    try: async (signal) => {
      const declaredBytes = Number(response.headers.get("content-length"));
      if (Number.isFinite(declaredBytes) && declaredBytes > maxBytes) {
        await response.body?.cancel().catch(() => undefined);
        throw new CaldavResponseTooLargeError(
          `response exceeds the ${maxBytes}-byte limit`,
        );
      }
      if (!response.body) return "";

      const reader = response.body.getReader();
      const abort = () => {
        void reader.cancel(signal.reason).catch(() => undefined);
      };
      signal.addEventListener("abort", abort, { once: true });

      const decoder = new TextDecoder();
      const parts: string[] = [];
      let receivedBytes = 0;
      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          receivedBytes += value.byteLength;
          if (receivedBytes > maxBytes) {
            await reader.cancel().catch(() => undefined);
            throw new CaldavResponseTooLargeError(
              `response exceeds the ${maxBytes}-byte limit`,
            );
          }
          parts.push(decoder.decode(value, { stream: true }));
        }
        parts.push(decoder.decode());
        return parts.join("");
      } finally {
        signal.removeEventListener("abort", abort);
        reader.releaseLock();
      }
    },
    catch: (cause) =>
      new CaldavError({
        operation,
        cause,
        transient: !(cause instanceof CaldavResponseTooLargeError),
      }),
  });
}

/**
 * Issue a CalDAV PROPFIND, following redirects manually — fetch() won't
 * replay a PROPFIND across a redirect, and iCloud's discovery chain redirects
 * from caldav.icloud.com to a per-account pXX-caldav.icloud.com shard.
 * Returns the multistatus XML plus the URL that finally answered (needed to
 * resolve relative hrefs against the right host).
 */
export function propfindEffect(
  url: string,
  authHeader: string,
  depth: "0" | "1",
  body: string,
): Effect.Effect<{ xml: string; url: string }, CaldavError> {
  return Effect.gen(function* () {
    let current = url;
    for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
      const response = yield* requestCaldavEffect(
        current,
        {
          method: "PROPFIND",
          redirect: "manual",
          headers: {
            "Content-Type": "application/xml; charset=utf-8",
            Authorization: authHeader,
            Depth: depth,
          },
          body,
        },
        "CalDAV PROPFIND",
      );

      if (response.status >= 300 && response.status < 400) {
        const location = response.headers.get("location");
        if (!location) {
          return yield* new CaldavError({
            operation: "CalDAV PROPFIND redirect",
            cause: `missing Location (${current})`,
            transient: false,
          });
        }
        const next = new URL(location, current);
        if (!isSafeRedirect(new URL(current), next)) {
          return yield* new CaldavError({
            operation: "CalDAV PROPFIND redirect",
            cause: `Refusing to forward CalDAV credentials across redirect (${new URL(current).origin} -> ${next.origin})`,
            transient: false,
          });
        }
        current = next.toString();
        continue;
      }

      if (!response.ok) {
        const text = yield* readCaldavResponseText(
          response,
          "read CalDAV PROPFIND error response",
          CALDAV_ERROR_MAX_BYTES,
        );
        return yield* new CaldavError({
          operation: "CalDAV PROPFIND",
          cause: `${response.status} ${response.statusText} (${current})\n${text}`,
          transient: response.status >= 500,
          statusCode: response.status,
        });
      }

      const rawXml = yield* readCaldavResponseText(
        response,
        "read CalDAV PROPFIND response",
        CALDAV_XML_MAX_BYTES,
      );
      const xml = yield* Schema.decodeUnknownEffect(CaldavXmlSchema)(rawXml).pipe(
        Effect.mapError(
          (cause) =>
            new CaldavError({
              operation: "decode CalDAV XML",
              cause,
              transient: false,
            }),
        ),
      );
      return { xml, url: current };
    }
    return yield* new CaldavError({
      operation: "CalDAV PROPFIND",
      cause: `exceeded ${MAX_REDIRECTS} redirects (${url})`,
      transient: false,
    });
  });
}

export function basicAuth(username: string, password: string): string {
  return `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`;
}
