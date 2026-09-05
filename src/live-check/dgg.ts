import type { Effect as EffectType } from "effect/Effect";
import WebSocket from "ws";
import { Data, Effect } from "effect";
import { z } from "zod";
import { canonicalBindingKey } from "./identityLinks.js";
import {
  type FetchedStatusLive,
  LiveStatus,
  Platform,
  platformConfigs,
} from "./platforms/index.js";
import { type DggPresence, normalizeId, type Streamer } from "./streamers.js";

export const DGG_LIVE_URL = "wss://live.destiny.gg";
const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const MAX_MESSAGE_BYTES = 2 * 1024 * 1024;

const supportedPlatformSchema = z.enum([
  Platform.YouTube,
  Platform.Twitch,
  Platform.Kick,
]);

const mediaMetadataSchema = z
  .object({
    previewUrl: z.string().nullable().optional(),
    displayName: z.string().min(1),
    title: z.string().nullable().optional(),
    createdDate: z.string().nullable().optional(),
    live: z.boolean(),
    viewers: z.number().int().nonnegative().nullable().optional(),
  })
  .passthrough();

const embedSchema = z
  .object({
    platform: z.string(),
    id: z.string().min(1),
    count: z.number().int().nonnegative(),
    mediaItem: z
      .object({
        identifier: z
          .object({
            platform: z.string(),
            mediaId: z.string().min(1),
          })
          .passthrough(),
        metadata: mediaMetadataSchema,
      })
      .passthrough(),
  })
  .passthrough();

export const dggEmbedsSchema = z.array(embedSchema);

export const dggHostingSchema = z
  .object({
    id: z.string().min(1),
    displayName: z.string().min(1),
    platform: z.string(),
    title: z.string().nullable().optional(),
    preview: z.string().nullable().optional(),
  })
  .passthrough()
  .nullable();

export const dggStreamInfoSchema = z
  .object({
    streams: z.record(
      z.string(),
      z.object({ live: z.boolean().optional() }).passthrough().nullable(),
    ),
  })
  .passthrough();

const envelopeSchema = z
  .object({
    type: z.string(),
    data: z.unknown(),
  })
  .passthrough();

export type DggEmbed = z.infer<typeof embedSchema>;
export type DggHosting = Exclude<z.infer<typeof dggHostingSchema>, null>;

export type DggFeed = {
  embeds: DggEmbed[];
  hosting: DggHosting | null;
  destinyLive: boolean;
};

export type DggFetchedStatus = FetchedStatusLive;

export type SelectedDggStream = {
  streamer: Streamer;
  status: DggFetchedStatus;
  /** Exact media/channel URL represented by the DGG entry. */
  url: string;
  previewUrl?: string;
  embedCount?: number;
  hosted: boolean;
  hosting?: DggHosting;
};

type WebSocketLike = {
  addEventListener(type: "open", listener: () => void): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
  addEventListener(type: "error", listener: () => void): void;
  addEventListener(type: "close", listener: () => void): void;
  close(): void;
};

export type DggWebSocketFactory = (url: string) => WebSocketLike;

function messageText(data: unknown): string | undefined {
  if (typeof data === "string") return data;
  if (data instanceof ArrayBuffer) return Buffer.from(data).toString("utf8");
  if (ArrayBuffer.isView(data)) {
    return Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString("utf8");
  }
  return undefined;
}

/**
 * Reads the initial DGG websocket snapshot. Both relevant messages are required,
 * including a null hosting message, so a partial snapshot can never erase the
 * last successful dynamic channel list.
 */
export class DggFeedError extends Data.TaggedError("DggFeedError")<{
  readonly message: string;
}> {}

export function fetchDggFeed({
  timeoutMs = 5_000,
  createSocket = (url) =>
    new WebSocket(url, {
      headers: { "User-Agent": USER_AGENT },
      handshakeTimeout: 5_000,
      maxPayload: MAX_MESSAGE_BYTES,
    }),
}: {
  timeoutMs?: number;
  createSocket?: DggWebSocketFactory;
} = {}): EffectType<DggFeed, DggFeedError> {
  const socketEffect = Effect.callback<DggFeed, DggFeedError>((resume) => {
    let embeds: DggEmbed[] | undefined;
    let hosting: DggHosting | null | undefined;
    let destinyLive: boolean | undefined;
    let settled = false;
    const socket = createSocket(DGG_LIVE_URL);

    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      socket.close();
      if (error) resume(Effect.fail(new DggFeedError({ message: error.message })));
      else
        resume(
          Effect.succeed({
            embeds: embeds ?? [],
            hosting: destinyLive ? null : (hosting ?? null),
            destinyLive: destinyLive ?? false,
          }),
        );
    };

    socket.addEventListener("message", (event) => {
      const text = messageText(event.data);
      if (text === undefined) return;

      let envelope: z.infer<typeof envelopeSchema>;
      try {
        envelope = envelopeSchema.parse(JSON.parse(text));
      } catch {
        return;
      }

      try {
        if (envelope.type === "dggApi:embeds") {
          embeds = dggEmbedsSchema.parse(envelope.data);
        } else if (envelope.type === "dggApi:hosting") {
          hosting = dggHostingSchema.parse(envelope.data);
        } else if (envelope.type === "dggApi:streamInfo") {
          const streamInfo = dggStreamInfoSchema.parse(envelope.data);
          destinyLive = Object.values(streamInfo.streams).some(
            (stream) => stream?.live === true,
          );
        } else {
          return;
        }
      } catch (error) {
        finish(
          new Error(`Invalid ${envelope.type} payload: ${(error as Error).message}`),
        );
        return;
      }

      if (embeds !== undefined && hosting !== undefined && destinyLive !== undefined) {
        finish();
      }
    });
    socket.addEventListener("error", () =>
      finish(new Error("DGG websocket connection failed")),
    );
    socket.addEventListener("close", () => {
      if (!settled)
        finish(new Error("DGG websocket closed before its snapshot arrived"));
    });
    return Effect.sync(() => {
      settled = true;
      socket.close();
    });
  });
  return socketEffect.pipe(
    Effect.timeoutOrElse({
      duration: `${timeoutMs} millis`,
      orElse: () =>
        Effect.fail(
          new DggFeedError({ message: "Timed out waiting for DGG live snapshot" }),
        ),
    }),
  );
}

function supportedPlatform(value: string): Platform | undefined {
  const parsed = supportedPlatformSchema.safeParse(value.toLowerCase());
  return parsed.success ? parsed.data : undefined;
}

export function canonicalBinding(platform: Platform, username: string): string {
  return canonicalBindingKey({ platform, username });
}

function streamUrl(platform: Platform, id: string): string {
  if (platform === Platform.YouTube) {
    return `https://www.youtube.com/watch?v=${encodeURIComponent(id)}`;
  }
  return platformConfigs[platform].getLiveUrl(id);
}

type Candidate = {
  platform: Platform;
  id: string;
  displayName: string;
  title: string;
  viewers?: number;
  startedAt?: string;
  previewUrl?: string;
  embedCount?: number;
  hosting?: DggHosting;
};

export type ResolvedDggStreams = {
  /** Unconfigured DGG sources that become transient background streamers. */
  discovered: SelectedDggStream[];
  /** DGG presence to overlay on configured streamers without changing behavior. */
  configuredPresence: Map<string, DggPresence>;
  /** DGG-provided platform observations attached to a configured identity. */
  configuredSources: Map<string, SelectedDggStream[]>;
};

/**
 * Resolves one DGG snapshot into configured-streamer enrichment and up to
 * `limit` additional background streamers. Configured matches never consume a
 * discovery slot, and every candidate is scanned so lower-ranked configured
 * sources retain their DGG metadata after the discovery limit is filled.
 */
export function resolveDggStreams({
  feed,
  limit,
  configuredStreamers,
  availablePlatforms,
  identityAliases = new Map(),
}: {
  feed: DggFeed;
  limit: number;
  configuredStreamers: readonly Streamer[];
  availablePlatforms: ReadonlySet<Platform>;
  /** Canonical source binding -> canonical configured binding. */
  identityAliases?: ReadonlyMap<string, string>;
}): ResolvedDggStreams {
  if (!Number.isInteger(limit) || limit < 0) {
    throw new Error(`DGG stream limit must be a non-negative integer, got ${limit}`);
  }

  const configuredByBinding = new Map<string, Streamer>();
  const configuredByName = new Map<string, Streamer>();
  const configuredByYouTubeName = new Map<string, Set<Streamer>>();
  const addYouTubeName = (name: string, streamer: Streamer) => {
    const key = normalizeId(name).replace(/^@/, "");
    const matches = configuredByYouTubeName.get(key) ?? new Set<Streamer>();
    matches.add(streamer);
    configuredByYouTubeName.set(key, matches);
  };
  for (const streamer of configuredStreamers) {
    configuredByName.set(normalizeId(streamer.displayName), streamer);
    addYouTubeName(streamer.displayName, streamer);
    for (const binding of streamer.bindings) {
      if (binding.platform === Platform.YouTube && binding.username.startsWith("@")) {
        addYouTubeName(binding.username, streamer);
      }
      configuredByBinding.set(
        canonicalBinding(binding.platform, binding.username),
        streamer,
      );
    }
  }

  const candidates: Candidate[] = [];
  if (!feed.destinyLive && feed.hosting) {
    const platform = supportedPlatform(feed.hosting.platform);
    if (platform && availablePlatforms.has(platform)) {
      const matchingEmbed = feed.embeds
        .filter((embed) => {
          const embedPlatform = supportedPlatform(embed.platform);
          return (
            embedPlatform === platform &&
            canonicalBinding(platform, embed.id) ===
              canonicalBinding(platform, feed.hosting?.id ?? "")
          );
        })
        .sort((a, b) => b.count - a.count)[0];
      candidates.push({
        platform,
        id: feed.hosting.id,
        displayName: feed.hosting.displayName,
        title: feed.hosting.title ?? `${feed.hosting.displayName} is hosted on DGG`,
        viewers: matchingEmbed?.mediaItem.metadata.viewers ?? undefined,
        startedAt: matchingEmbed?.mediaItem.metadata.createdDate ?? undefined,
        previewUrl: feed.hosting.preview ?? undefined,
        embedCount: matchingEmbed?.count,
        hosting: feed.hosting,
      });
    }
  }

  for (const embed of [...feed.embeds].sort((a, b) => b.count - a.count)) {
    const platform = supportedPlatform(embed.platform);
    const mediaPlatform = supportedPlatform(embed.mediaItem.identifier.platform);
    if (
      !platform ||
      mediaPlatform !== platform ||
      !availablePlatforms.has(platform) ||
      !embed.mediaItem.metadata.live
    ) {
      continue;
    }
    candidates.push({
      platform,
      id: embed.mediaItem.identifier.mediaId,
      displayName: embed.mediaItem.metadata.displayName,
      title: embed.mediaItem.metadata.title ?? embed.mediaItem.metadata.displayName,
      viewers: embed.mediaItem.metadata.viewers ?? undefined,
      startedAt: embed.mediaItem.metadata.createdDate ?? undefined,
      previewUrl: embed.mediaItem.metadata.previewUrl ?? undefined,
      embedCount: embed.count,
    });
  }

  // Hosting is one more DGG placement, not a priority lane. Rank every source
  // by the number of DGG viewers before applying the configured-channel filter
  // and discovery limit. Stable sorting keeps the richer hosting candidate
  // first when it duplicates an embed with the same count.
  candidates.sort((a, b) => (b.embedCount ?? 0) - (a.embedCount ?? 0));

  const seen = new Set<string>();
  const discovered: SelectedDggStream[] = [];
  const configuredPresence = new Map<string, DggPresence>();
  const configuredSources = new Map<string, SelectedDggStream[]>();
  for (const candidate of candidates) {
    const binding = canonicalBinding(candidate.platform, candidate.id);
    if (seen.has(binding)) continue;
    seen.add(binding);

    const aliasTarget = identityAliases.get(binding);
    const configuredExact = configuredByBinding.get(binding);
    const configuredAlias = aliasTarget
      ? configuredByBinding.get(aliasTarget)
      : undefined;
    // DGG identifies YouTube streams by video ID, but may name their owner
    // with the channel handle instead of our configured display name.
    // Match only an unambiguous name, and keep polling the configured account
    // so the same YouTube audience is not counted again as a linked source.
    const youtubeMatches = configuredByYouTubeName.get(
      normalizeId(candidate.displayName).replace(/^@/, ""),
    );
    const configuredName =
      candidate.platform === Platform.YouTube
        ? youtubeMatches?.size === 1
          ? [...youtubeMatches][0]
          : undefined
        : configuredByName.get(normalizeId(candidate.displayName));
    const configured = configuredExact ?? configuredAlias ?? configuredName;
    const url = streamUrl(candidate.platform, candidate.id);
    const selected: SelectedDggStream = {
      streamer: {
        id: `dgg:${candidate.platform}:${encodeURIComponent(candidate.id.toLowerCase())}`,
        displayName: candidate.displayName,
        bindings: [
          { platform: candidate.platform, username: candidate.id, urlOverride: url },
        ],
        tier: "background",
        discoverySource: "dgg",
        dgg: {
          hosted: candidate.hosting !== undefined,
          viewers: candidate.embedCount ?? null,
        },
      },
      status: {
        status: LiveStatus.Live,
        title: candidate.title,
        viewerCount: candidate.viewers,
        startedAt: candidate.startedAt,
      },
      url,
      previewUrl: candidate.previewUrl,
      embedCount: candidate.embedCount,
      hosted: candidate.hosting !== undefined,
      hosting: candidate.hosting,
    };
    if (configured) {
      const previous = configuredPresence.get(configured.id);
      const viewers =
        candidate.embedCount === undefined
          ? (previous?.viewers ?? null)
          : (previous?.viewers ?? 0) + candidate.embedCount;
      configuredPresence.set(configured.id, {
        hosted: previous?.hosted === true || candidate.hosting !== undefined,
        viewers,
      });
      if (!configuredExact && configuredAlias) {
        const sources = configuredSources.get(configured.id) ?? [];
        sources.push(selected);
        configuredSources.set(configured.id, sources);
      }
      continue;
    }
    if (discovered.length >= limit) continue;

    discovered.push(selected);
  }

  return { discovered, configuredPresence, configuredSources };
}
