import got from "got";
import { Effect, Schema } from "effect";
import { integrationEffect, RecommendationIntegrationError } from "../effect.js";
import {
  type ExternalIds,
  type InProgressItem,
  type MediaItem,
  MediaType,
  type WatchedItem,
} from "../types.js";

const PlexGuidSchema = Schema.Struct({ id: Schema.optional(Schema.String) });
const PlexDirectorySchema = Schema.Struct({
  key: Schema.optional(Schema.String),
  type: Schema.optional(Schema.String),
});
const PlexMetadataSchema = Schema.Struct({
  type: Schema.optional(Schema.String),
  ratingKey: Schema.optional(Schema.String),
  guid: Schema.optional(Schema.String),
  title: Schema.optional(Schema.String),
  year: Schema.optional(Schema.Number),
  duration: Schema.optional(Schema.Number),
  viewOffset: Schema.optional(Schema.Number),
  viewedAt: Schema.optional(Schema.Number),
  lastViewedAt: Schema.optional(Schema.Number),
  viewCount: Schema.optional(Schema.Number),
  leafCount: Schema.optional(Schema.Number),
  viewedLeafCount: Schema.optional(Schema.Number),
  accountID: Schema.optional(Schema.Number),
  grandparentRatingKey: Schema.optional(Schema.String),
  grandparentKey: Schema.optional(Schema.String),
  grandparentGuid: Schema.optional(Schema.String),
  grandparentTitle: Schema.optional(Schema.String),
  grandparentYear: Schema.optional(Schema.Number),
  Guid: Schema.optional(Schema.Array(PlexGuidSchema)),
});
const PlexContainerSchema = Schema.Struct({
  size: Schema.optional(Schema.Number),
  totalSize: Schema.optional(Schema.Number),
  Metadata: Schema.optional(Schema.Array(PlexMetadataSchema)),
  Directory: Schema.optional(Schema.Array(PlexDirectorySchema)),
});
const PlexResponseSchema = Schema.Struct({ MediaContainer: PlexContainerSchema });

type PlexContainer = Schema.Schema.Type<typeof PlexContainerSchema>;
type PlexMetadata = Schema.Schema.Type<typeof PlexMetadataSchema>;

export type PlexGet = (
  path: string,
  searchParams?: Record<string, string | number>,
) => Promise<unknown>;

function decodeContainer(
  operation: string,
  value: unknown,
): Effect.Effect<PlexContainer, RecommendationIntegrationError> {
  return Schema.decodeUnknown(PlexResponseSchema)(value).pipe(
    Effect.map((decoded) => decoded.MediaContainer),
    Effect.mapError(
      (cause) =>
        new RecommendationIntegrationError({
          operation: `decode ${operation}`,
          cause,
        }),
    ),
  );
}

function fraction(offset?: number, duration?: number): number | undefined {
  if (offset === undefined || !duration || duration <= 0) return undefined;
  return Math.min(1, Math.max(0, offset / duration));
}

/** Handles both modern Guid arrays and legacy Plex agent GUIDs. */
export function parseExternalIds(metadata: PlexMetadata): ExternalIds | undefined {
  const ids: ExternalIds = {};
  const guids = [
    metadata.guid,
    ...(metadata.Guid ?? []).map((entry) => entry.id),
  ].filter((guid): guid is string => Boolean(guid));
  for (const guid of guids) {
    const tmdb = guid.match(/(?:tmdb|themoviedb)(?::\/\/|\/)(\d+)/i)?.[1];
    const imdb = guid.match(/imdb(?::\/\/|\/)(tt\d+)/i)?.[1];
    const tvdb = guid.match(/(?:tvdb|thetvdb)(?::\/\/|\/)(\d+)/i)?.[1];
    if (tmdb) ids.tmdb = Number(tmdb);
    if (imdb) ids.imdb = imdb;
    if (tvdb) ids.tvdb = Number(tvdb);
  }
  return Object.keys(ids).length > 0 ? ids : undefined;
}

function nativeGuid(metadata: PlexMetadata): string {
  return (
    metadata.guid ??
    (metadata.ratingKey ? `plex://${metadata.type}/${metadata.ratingKey}` : "")
  );
}

function mediaItem(
  metadata: PlexMetadata,
  mediaType: MediaType,
): MediaItem | undefined {
  const guid = nativeGuid(metadata);
  if (!guid || !metadata.title) return undefined;
  return {
    guid,
    title: metadata.title,
    year: metadata.year,
    mediaType,
    externalIds: parseExternalIds(metadata),
  };
}

function seriesKey(metadata: PlexMetadata): string | undefined {
  return (
    metadata.grandparentRatingKey ??
    metadata.grandparentKey?.match(/^\/library\/metadata\/(\d+)(?:\/|$)/)?.[1] ??
    metadata.grandparentGuid
  );
}

function episodeSeries(
  metadata: PlexMetadata,
  detail?: PlexMetadata,
): MediaItem | undefined {
  const title = detail?.title ?? metadata.grandparentTitle;
  const guid = detail ? nativeGuid(detail) : (metadata.grandparentGuid ?? "");
  if (!title || !guid) return undefined;
  return {
    guid,
    title,
    year: detail?.year ?? metadata.grandparentYear,
    mediaType: MediaType.Tv,
    externalIds: detail ? parseExternalIds(detail) : undefined,
  };
}

function timestamp(metadata: PlexMetadata): number {
  // Plex timestamps are Unix seconds; recommendation timestamps are epoch ms.
  return (metadata.viewedAt ?? metadata.lastViewedAt ?? 0) * 1000;
}

function seriesProgress(
  detail: PlexMetadata | undefined,
  currentEpisodeProgress = 0,
): number | undefined {
  if (!detail?.leafCount || detail.leafCount <= 0) return undefined;
  return Math.min(
    1,
    Math.max(
      0,
      ((detail.viewedLeafCount ?? 0) + currentEpisodeProgress) / detail.leafCount,
    ),
  );
}

export class PlexClient {
  public constructor(
    private readonly get: PlexGet,
    private readonly accountId?: number,
  ) {}

  private metadataDetailsEffect(keys: (string | undefined)[]) {
    const uniqueKeys = [...new Set(keys.filter((key): key is string => Boolean(key)))];
    return Effect.forEach(
      uniqueKeys,
      (key): Effect.Effect<readonly [string, PlexMetadata | undefined]> => {
        if (!/^\d+$/.test(key)) return Effect.succeed([key, undefined] as const);
        return integrationEffect(`Plex metadata ${key}`, () =>
          this.get(`/library/metadata/${key}`, { includeGuids: 1 }),
        ).pipe(
          Effect.flatMap((value) => decodeContainer(`Plex metadata ${key}`, value)),
          Effect.map((container) => [key, container.Metadata?.[0]] as const),
          Effect.catchAll(() => Effect.succeed([key, undefined] as const)),
        );
      },
      { concurrency: 6 },
    ).pipe(
      Effect.map(
        (pairs) =>
          new Map(
            pairs.filter((pair): pair is readonly [string, PlexMetadata] =>
              Boolean(pair[1]),
            ),
          ),
      ),
    );
  }

  private showDetailsEffect(metadata: readonly PlexMetadata[]) {
    return this.metadataDetailsEffect(metadata.map(seriesKey));
  }

  public fetchWatchHistory(): Effect.Effect<
    WatchedItem[],
    RecommendationIntegrationError
  > {
    return Effect.gen(this, function* () {
      const metadata: PlexMetadata[] = [];
      const size = 100;
      for (let start = 0; ; start += size) {
        const raw = yield* integrationEffect("Plex watch history", () =>
          this.get("/status/sessions/history/all", {
            sort: "viewedAt:desc",
            "X-Plex-Container-Start": start,
            "X-Plex-Container-Size": size,
            includeGuids: 1,
            ...(this.accountId ? { accountID: this.accountId } : {}),
          }),
        );
        const container = yield* decodeContainer("Plex watch history", raw);
        const page = container.Metadata ?? [];
        metadata.push(...page);
        const total = container.totalSize ?? container.size ?? page.length;
        if (page.length === 0 || start + page.length >= total) break;
      }

      const accountIds = new Set(
        metadata
          .map((item) => item.accountID)
          .filter((id): id is number => Number.isInteger(id)),
      );
      if (!this.accountId && accountIds.size > 1) {
        return yield* Effect.fail(
          new RecommendationIntegrationError({
            operation: "validate Plex watch history account scope",
            cause: new Error(
              "Plex history contains multiple accounts; configure PLEX_ACCOUNT_ID",
            ),
          }),
        );
      }

      const episodes = metadata.filter((item) => item.type === "episode");
      const [details, movieDetails] = yield* Effect.all(
        [
          this.showDetailsEffect(episodes),
          this.metadataDetailsEffect(
            metadata
              .filter((item) => item.type === "movie")
              .map((item) => item.ratingKey),
          ),
        ] as const,
        { concurrency: "unbounded" },
      );
      const watched: WatchedItem[] = [];
      const shows = new Map<string, { item: WatchedItem; detailKey: string }>();

      for (const entry of metadata) {
        if (entry.type === "movie") {
          const detail = movieDetails.get(entry.ratingKey ?? "");
          const item = mediaItem(detail ?? entry, MediaType.Movie);
          if (!item) continue;
          watched.push({
            ...item,
            viewedAt: timestamp(entry),
            viewCount: entry.viewCount ?? 1,
            completion: fraction(entry.viewOffset, entry.duration),
          });
        } else if (entry.type === "episode") {
          const key = seriesKey(entry);
          if (!key) continue;
          const item = episodeSeries(entry, details.get(key));
          if (!item) continue;
          const existing = shows.get(item.guid);
          if (!existing) {
            shows.set(item.guid, {
              item: {
                ...item,
                viewedAt: timestamp(entry),
                // Episode play counts cannot establish that the whole series
                // was rewatched, so keep series-level rewatch evidence neutral.
                viewCount: 1,
              },
              detailKey: key,
            });
          } else {
            existing.item.viewedAt = Math.max(existing.item.viewedAt, timestamp(entry));
            // Keep viewCount at 1: an episode replay is not a series replay.
          }
        }
      }
      for (const aggregate of shows.values()) {
        const detail = details.get(aggregate.detailKey);
        aggregate.item.completion = seriesProgress(detail);
        watched.push(aggregate.item);
      }
      return watched;
    });
  }

  public fetchInProgress(): Effect.Effect<
    InProgressItem[],
    RecommendationIntegrationError
  > {
    return Effect.gen(this, function* () {
      const raw = yield* integrationEffect("Plex continue watching", () =>
        this.get("/hubs/home/continueWatching", { includeGuids: 1 }),
      );
      const container = yield* decodeContainer("Plex continue watching", raw);
      const metadata = container.Metadata ?? [];
      const episodes = metadata.filter((item) => item.type === "episode");
      const details = yield* this.showDetailsEffect(episodes);
      const items = new Map<string, InProgressItem>();
      for (const entry of metadata) {
        const episodeProgress = fraction(entry.viewOffset, entry.duration);
        if (
          episodeProgress === undefined ||
          episodeProgress <= 0 ||
          episodeProgress >= 1
        )
          continue;
        const detail =
          entry.type === "episode" ? details.get(seriesKey(entry) ?? "") : undefined;
        const progress =
          entry.type === "episode"
            ? (seriesProgress(detail, episodeProgress) ?? episodeProgress)
            : episodeProgress;
        const item =
          entry.type === "movie"
            ? mediaItem(entry, MediaType.Movie)
            : entry.type === "episode"
              ? episodeSeries(entry, details.get(seriesKey(entry) ?? ""))
              : undefined;
        if (!item) continue;
        const prior = items.get(item.guid);
        const lastViewedAt = timestamp(entry);
        if (!prior || lastViewedAt >= prior.lastViewedAt) {
          items.set(item.guid, { ...item, progress, lastViewedAt });
        }
      }
      return [...items.values()];
    });
  }

  public fetchLibraryIndex(): Effect.Effect<
    MediaItem[],
    RecommendationIntegrationError
  > {
    return Effect.gen(this, function* () {
      const sectionsRaw = yield* integrationEffect("Plex library sections", () =>
        this.get("/library/sections"),
      );
      const sectionContainer = yield* decodeContainer(
        "Plex library sections",
        sectionsRaw,
      );
      const sections = (sectionContainer.Directory ?? []).filter(
        (section) =>
          section.key && (section.type === "movie" || section.type === "show"),
      );
      const containers = yield* Effect.forEach(
        sections,
        (section) =>
          Effect.gen(this, function* () {
            const metadata: PlexMetadata[] = [];
            const size = 500;
            for (let start = 0; ; start += size) {
              const raw = yield* integrationEffect(
                `Plex library section ${section.key}`,
                () =>
                  this.get(`/library/sections/${section.key}/all`, {
                    includeGuids: 1,
                    "X-Plex-Container-Start": start,
                    "X-Plex-Container-Size": size,
                  }),
              );
              const container = yield* decodeContainer(
                `Plex library section ${section.key}`,
                raw,
              );
              const page = container.Metadata ?? [];
              metadata.push(...page);
              const total = container.totalSize ?? container.size ?? page.length;
              if (page.length === 0 || start + page.length >= total) break;
            }
            return metadata;
          }),
        { concurrency: 4 },
      );
      const items: MediaItem[] = [];
      for (const metadata of containers) {
        for (const entry of metadata) {
          const mediaType =
            entry.type === "movie"
              ? MediaType.Movie
              : entry.type === "show"
                ? MediaType.Tv
                : undefined;
          if (!mediaType) continue;
          const item = mediaItem(entry, mediaType);
          if (item) items.push(item);
        }
      }
      return items;
    });
  }
}

export function createPlexClient(
  url?: string,
  token?: string,
  accountId?: number,
): PlexClient {
  if (!url) throw new Error("PLEX_URL is not configured");
  if (!token) throw new Error("PLEX_TOKEN is not configured");
  const baseUrl = url.replace(/\/$/, "");
  return new PlexClient(
    (path, searchParams = {}) =>
      got
        .get(`${baseUrl}${path}`, {
          searchParams,
          headers: { Accept: "application/json", "X-Plex-Token": token },
          timeout: { request: 15_000 },
          retry: { limit: 2 },
        })
        .json<unknown>(),
    accountId,
  );
}
