import got from "got";
import PQueue from "p-queue";
import {
  type ExternalIds,
  type InProgressItem,
  type MediaItem,
  MediaType,
  type WatchedItem,
} from "../types.js";

interface PlexContainer {
  size?: number;
  totalSize?: number;
  Metadata?: PlexMetadata[];
  Directory?: PlexDirectory[];
}

interface PlexResponse {
  MediaContainer?: PlexContainer;
}

interface PlexDirectory {
  key?: string;
  type?: string;
}

interface PlexGuid {
  id?: string;
}

interface PlexMetadata {
  type?: string;
  ratingKey?: string;
  guid?: string;
  title?: string;
  year?: number;
  duration?: number;
  viewOffset?: number;
  viewedAt?: number;
  lastViewedAt?: number;
  viewCount?: number;
  leafCount?: number;
  viewedLeafCount?: number;
  accountID?: number;
  grandparentRatingKey?: string;
  grandparentGuid?: string;
  grandparentTitle?: string;
  grandparentYear?: number;
  Guid?: PlexGuid[];
}

export type PlexGet = (
  path: string,
  searchParams?: Record<string, string | number>,
) => Promise<unknown>;

function asContainer(value: unknown): PlexContainer {
  if (!value || typeof value !== "object")
    throw new Error("Plex returned an invalid response");
  const container = (value as PlexResponse).MediaContainer;
  if (!container || typeof container !== "object") {
    throw new Error("Plex response did not contain a MediaContainer");
  }
  return container;
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
  return metadata.grandparentRatingKey ?? metadata.grandparentGuid;
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

  private async metadataDetails(keys: (string | undefined)[]) {
    const uniqueKeys = [...new Set(keys.filter((key): key is string => Boolean(key)))];
    const queue = new PQueue({ concurrency: 6 });
    const pairs = await Promise.all(
      uniqueKeys.map((key) =>
        queue.add(async (): Promise<[string, PlexMetadata | undefined]> => {
          // A grandparent GUID is still useful for identity when Plex did not
          // provide a rating key, but only rating keys can be dereferenced.
          if (!/^\d+$/.test(key)) return [key, undefined];
          try {
            const container = asContainer(
              await this.get(`/library/metadata/${key}`, { includeGuids: 1 }),
            );
            return [key, container.Metadata?.[0]];
          } catch {
            return [key, undefined];
          }
        }),
      ),
    );
    return new Map(
      pairs.filter((pair): pair is [string, PlexMetadata] => Boolean(pair[1])),
    );
  }

  private showDetails(metadata: PlexMetadata[]): Promise<Map<string, PlexMetadata>> {
    return this.metadataDetails(metadata.map(seriesKey));
  }

  public async fetchWatchHistory(): Promise<WatchedItem[]> {
    const metadata: PlexMetadata[] = [];
    const size = 100;
    for (let start = 0; ; start += size) {
      const container = asContainer(
        await this.get("/status/sessions/history/all", {
          sort: "viewedAt:desc",
          "X-Plex-Container-Start": start,
          "X-Plex-Container-Size": size,
          includeGuids: 1,
          ...(this.accountId ? { accountID: this.accountId } : {}),
        }),
      );
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
      throw new Error(
        "Plex history contains multiple accounts; configure PLEX_ACCOUNT_ID",
      );
    }

    const episodes = metadata.filter((item) => item.type === "episode");
    const [details, movieDetails] = await Promise.all([
      this.showDetails(episodes),
      this.metadataDetails(
        metadata.filter((item) => item.type === "movie").map((item) => item.ratingKey),
      ),
    ]);
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
  }

  public async fetchInProgress(): Promise<InProgressItem[]> {
    const container = asContainer(
      await this.get("/hubs/home/continueWatching", { includeGuids: 1 }),
    );
    const metadata = container.Metadata ?? [];
    const episodes = metadata.filter((item) => item.type === "episode");
    const details = await this.showDetails(episodes);
    const items = new Map<string, InProgressItem>();
    for (const entry of metadata) {
      const episodeProgress = fraction(entry.viewOffset, entry.duration);
      if (episodeProgress === undefined || episodeProgress <= 0 || episodeProgress >= 1)
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
  }

  public async fetchLibraryIndex(): Promise<MediaItem[]> {
    const sectionContainer = asContainer(await this.get("/library/sections"));
    const sections = (sectionContainer.Directory ?? []).filter(
      (section) => section.key && (section.type === "movie" || section.type === "show"),
    );
    const containers = await Promise.all(
      sections.map(async (section) => {
        const metadata: PlexMetadata[] = [];
        const size = 500;
        for (let start = 0; ; start += size) {
          const container = asContainer(
            await this.get(`/library/sections/${section.key}/all`, {
              includeGuids: 1,
              "X-Plex-Container-Start": start,
              "X-Plex-Container-Size": size,
            }),
          );
          const page = container.Metadata ?? [];
          metadata.push(...page);
          const total = container.totalSize ?? container.size ?? page.length;
          if (page.length === 0 || start + page.length >= total) break;
        }
        return metadata;
      }),
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
    async (path, searchParams = {}) =>
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
