import { decode } from "html-entities";
import { Clock, Data, Effect } from "effect";
import { parseHTML } from "linkedom";
import type { PersistenceError } from "../effect/errors.js";
import {
  canonicalBindingKey,
  getProfileIdentityLinkEffect,
  type ProfileIdentityLink,
  rememberProfileIdentityLinkEffect,
} from "./identityLinks.js";
import { Platform } from "./platforms/index.js";
import type { PlatformBinding } from "./streamers.js";

const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const DEFAULT_TIMEOUT_MS = 5_000;
const DEFAULT_MAX_BYTES = 2_000_000;

const RESERVED_KICK_PATHS = new Set([
  "api",
  "browse",
  "categories",
  "category",
  "following",
  "search",
]);
const RESERVED_TWITCH_PATHS = new Set([
  "directory",
  "downloads",
  "jobs",
  "p",
  "settings",
  "videos",
]);

export type ProfilePageFetcher = (
  input: string | URL,
  init?: RequestInit,
) => Promise<Response>;

export class ProfileLinkError extends Data.TaggedError("ProfileLinkError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation}: ${detail}`;
  }
}

function validHandle(value: string): boolean {
  return /^[A-Za-z0-9_.-]{2,100}$/.test(value);
}

function unwrapYouTubeRedirect(url: URL): URL | undefined {
  if (
    (url.hostname === "youtube.com" || url.hostname === "www.youtube.com") &&
    url.pathname === "/redirect"
  ) {
    const destination = url.searchParams.get("q");
    if (!destination) return undefined;
    try {
      return new URL(destination);
    } catch {
      return undefined;
    }
  }
  return url;
}

/** Accepts account/profile URLs only. Video, category, and arbitrary links are rejected. */
export function bindingFromProfileUrl(rawUrl: string): PlatformBinding | undefined {
  let parsed: URL;
  try {
    parsed = new URL(decode(rawUrl).replaceAll("\\/", "/"));
  } catch {
    return undefined;
  }

  const url = unwrapYouTubeRedirect(parsed);
  if (!url || (url.protocol !== "https:" && url.protocol !== "http:")) {
    return undefined;
  }
  if (url.username || url.password || url.port) return undefined;
  const host = url.hostname.toLowerCase().replace(/^www\./, "");
  const parts = url.pathname.split("/").filter(Boolean);

  if (host === "kick.com" && parts.length === 1) {
    const username = parts[0];
    if (!validHandle(username) || RESERVED_KICK_PATHS.has(username.toLowerCase())) {
      return undefined;
    }
    return { platform: Platform.Kick, username: username.toLowerCase() };
  }

  if (host === "twitch.tv" && parts.length === 1) {
    const username = parts[0];
    if (!validHandle(username) || RESERVED_TWITCH_PATHS.has(username.toLowerCase())) {
      return undefined;
    }
    return { platform: Platform.Twitch, username: username.toLowerCase() };
  }

  if (host === "youtube.com") {
    if (parts.length === 1 && parts[0].startsWith("@")) {
      const handle = parts[0].slice(1);
      if (validHandle(handle)) {
        return { platform: Platform.YouTube, username: `@${handle.toLowerCase()}` };
      }
    }
    if (
      parts.length === 2 &&
      ["channel", "c", "user"].includes(parts[0].toLowerCase()) &&
      validHandle(parts[1])
    ) {
      return {
        platform: Platform.YouTube,
        username: `${parts[0].toLowerCase()}/${parts[1]}`,
      };
    }
  }
  return undefined;
}

/** Extract supported direct profile links from anchors and embedded structured JSON. */
export function extractProfileLinks(html: string): PlatformBinding[] {
  const candidates: string[] = [];
  const { document } = parseHTML(html);
  for (const anchor of document.querySelectorAll("a[href]")) {
    const href = anchor.getAttribute("href");
    if (href) candidates.push(href);
  }

  // YouTube and Kick serialize external links into page JSON as escaped URLs.
  // Keep this deliberately URL-shaped rather than parsing or crawling arbitrary links.
  for (const match of html.matchAll(
    /https?:\\?\/\\?\/(?:www\\?\.)?(?:youtube\\?\.com|kick\\?\.com|twitch\\?\.tv)[^"'<>\s]*/gi,
  )) {
    candidates.push(match[0]);
  }

  const byKey = new Map<string, PlatformBinding>();
  for (const candidate of candidates) {
    const binding = bindingFromProfileUrl(candidate);
    if (binding) byKey.set(canonicalBindingKey(binding), binding);
  }
  return [...byKey.values()];
}

export function profilePageUrl(binding: PlatformBinding): string | undefined {
  const username = binding.username.trim();
  if (!username) return undefined;
  switch (binding.platform) {
    case Platform.YouTube:
      if (username.startsWith("@") && validHandle(username.slice(1))) {
        return `https://www.youtube.com/@${username.slice(1).toLowerCase()}/about`;
      }
      {
        const parts = username.split("/");
        if (
          parts.length === 2 &&
          ["channel", "c", "user"].includes(parts[0].toLowerCase()) &&
          validHandle(parts[1])
        ) {
          return `https://www.youtube.com/${parts[0].toLowerCase()}/${parts[1]}/about`;
        }
      }
      return undefined;
    case Platform.Kick:
      return validHandle(username)
        ? `https://kick.com/${username.toLowerCase()}`
        : undefined;
    case Platform.Twitch:
      return validHandle(username)
        ? `https://www.twitch.tv/${username.toLowerCase()}/about`
        : undefined;
  }
}

function readBoundedText(
  response: Response,
  maxBytes: number,
): Effect.Effect<string, ProfileLinkError> {
  const readError = (cause: unknown) =>
    new ProfileLinkError({ operation: "read bounded profile page", cause });

  return Effect.gen(function* () {
    if (!response.ok) {
      return yield* readError(
        new Error(`Profile page returned HTTP ${response.status}`),
      );
    }
    const declaredLength = Number(response.headers.get("content-length"));
    if (Number.isFinite(declaredLength) && declaredLength > maxBytes) {
      return yield* readError(new Error(`Profile page exceeds ${maxBytes} byte limit`));
    }
    if (!response.body) return "";

    return yield* Effect.acquireUseRelease(
      Effect.try({
        try: () => response.body!.getReader(),
        catch: readError,
      }),
      (reader) =>
        Effect.tryPromise({
          try: async () => {
            const chunks: Uint8Array[] = [];
            let bytes = 0;
            while (true) {
              const { done, value } = await reader.read();
              if (done) break;
              bytes += value.byteLength;
              if (bytes > maxBytes) {
                throw new Error(`Profile page exceeds ${maxBytes} byte limit`);
              }
              chunks.push(value);
            }
            const combined = new Uint8Array(bytes);
            let offset = 0;
            for (const chunk of chunks) {
              combined.set(chunk, offset);
              offset += chunk.byteLength;
            }
            return new TextDecoder().decode(combined);
          },
          catch: readError,
        }),
      (reader) =>
        Effect.tryPromise({
          try: () => reader.cancel(),
          catch: readError,
        }).pipe(
          Effect.ignore,
          Effect.ensuring(
            Effect.try({
              try: () => reader.releaseLock(),
              catch: readError,
            }).pipe(Effect.ignore),
          ),
        ),
    );
  });
}

/** Fetches exactly the canonical page for the supplied supported account. */
export function fetchProfileLinksEffect(
  binding: PlatformBinding,
  {
    fetchImpl = fetch,
    timeoutMs = DEFAULT_TIMEOUT_MS,
    maxBytes = DEFAULT_MAX_BYTES,
  }: {
    fetchImpl?: ProfilePageFetcher;
    timeoutMs?: number;
    maxBytes?: number;
  } = {},
): Effect.Effect<PlatformBinding[], ProfileLinkError> {
  const url = profilePageUrl(binding);
  if (!url) return Effect.succeed([]);
  return Effect.tryPromise({
    try: (signal) =>
      fetchImpl(url, {
        headers: {
          Accept: "text/html,application/xhtml+xml",
          "User-Agent": USER_AGENT,
        },
        // Refuse redirects so a platform-controlled response cannot turn this
        // bounded crawler into a fetch of an unrelated host.
        redirect: "error",
        signal: AbortSignal.any([signal, AbortSignal.timeout(timeoutMs)]),
      }),
    catch: (cause) =>
      new ProfileLinkError({ operation: `fetch profile page ${url}`, cause }),
  }).pipe(
    Effect.flatMap((response) => readBoundedText(response, maxBytes)),
    Effect.map(extractProfileLinks),
  );
}

export function fetchProfileLinks(
  binding: PlatformBinding,
  options: {
    fetchImpl?: ProfilePageFetcher;
    timeoutMs?: number;
    maxBytes?: number;
  } = {},
): Promise<PlatformBinding[]> {
  return Effect.runPromise(fetchProfileLinksEffect(binding, options));
}

function normalizedHandle(binding: PlatformBinding): string | undefined {
  const username = binding.username.trim().toLowerCase().replace(/^@/, "");
  return username.includes("/") ? undefined : username;
}

export type ProfileIdentityEvidence = "equal-handle" | "reciprocal";

/** Pure confidence gate used before any durable alias is written. */
export function profileIdentityEvidence({
  source,
  target,
  directLinks,
  reciprocalLinks = [],
}: {
  source: PlatformBinding;
  target: PlatformBinding;
  directLinks: readonly PlatformBinding[];
  reciprocalLinks?: readonly PlatformBinding[];
}): ProfileIdentityEvidence | undefined {
  const sourceKey = canonicalBindingKey(source);
  const targetKey = canonicalBindingKey(target);
  if (!directLinks.some((link) => canonicalBindingKey(link) === targetKey)) {
    return undefined;
  }

  const sourceHandle = normalizedHandle(source);
  if (sourceHandle && sourceHandle === normalizedHandle(target)) {
    return "equal-handle";
  }
  if (reciprocalLinks.some((link) => canonicalBindingKey(link) === sourceKey)) {
    return "reciprocal";
  }
  return undefined;
}

/**
 * Learns a durable source -> configured-binding alias from deterministic profile
 * evidence: a direct link with equal handles, or reciprocal direct profile links.
 */
export function learnProfileIdentityEffect({
  source,
  configuredBindings,
  fetchImpl = fetch,
  now,
  forceRefresh = false,
}: {
  source: PlatformBinding;
  configuredBindings: readonly PlatformBinding[];
  fetchImpl?: ProfilePageFetcher;
  now?: number;
  forceRefresh?: boolean;
}): Effect.Effect<
  ProfileIdentityLink | undefined,
  ProfileLinkError | PersistenceError
> {
  return Effect.gen(function* () {
    const observedAt = now ?? (yield* Clock.currentTimeMillis);
    const configuredByKey = new Map(
      configuredBindings.map((binding) => [canonicalBindingKey(binding), binding]),
    );
    const existing = yield* getProfileIdentityLinkEffect(source);
    if (!forceRefresh && existing && configuredByKey.has(existing.targetBinding)) {
      return existing;
    }
    const sourceKey = canonicalBindingKey(source);
    const directLinks = yield* fetchProfileLinksEffect(source, { fetchImpl });
    for (const directTarget of directLinks) {
      const configuredTarget = configuredByKey.get(canonicalBindingKey(directTarget));
      if (!configuredTarget || canonicalBindingKey(configuredTarget) === sourceKey) {
        continue;
      }

      if (
        profileIdentityEvidence({
          source,
          target: configuredTarget,
          directLinks,
        }) === "equal-handle"
      ) {
        return yield* rememberProfileIdentityLinkEffect({
          source,
          target: configuredTarget,
          now: observedAt,
        });
      }

      const reciprocalLinks = yield* fetchProfileLinksEffect(configuredTarget, {
        fetchImpl,
      });
      if (
        profileIdentityEvidence({
          source,
          target: configuredTarget,
          directLinks,
          reciprocalLinks,
        }) === "reciprocal"
      ) {
        return yield* rememberProfileIdentityLinkEffect({
          source,
          target: configuredTarget,
          now: observedAt,
        });
      }
    }
    return undefined;
  });
}

export function learnProfileIdentity(
  input: Parameters<typeof learnProfileIdentityEffect>[0],
): Promise<ProfileIdentityLink | undefined> {
  return Effect.runPromise(learnProfileIdentityEffect(input));
}
