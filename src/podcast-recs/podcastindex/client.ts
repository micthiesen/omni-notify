import type { Logger } from "@micthiesen/mitools/logging";
import got from "got";
import PQueue from "p-queue";
import { z } from "zod";
import config from "../../utils/config.js";
import { type PodcastIndexCredentials, podcastIndexAuthHeaders } from "./auth.js";
import type { PodcastIndexEpisode } from "./types.js";

const BASE_URL = "https://api.podcastindex.org/api/1.0";
const DEFAULT_MAX_RESULTS = 20;

const MAX_CONCURRENT_REQUESTS = 4;
const MAX_REQUESTS_PER_INTERVAL = 6;
const RATE_INTERVAL_MS = 1000;

// Tolerate whatever extra fields the API adds/renames over time — we only
// pull the handful of fields we care about, and every one of them is
// optional here so a missing/renamed field degrades to a skip rather than a
// thrown parse error.
const rawEpisodeSchema = z
  .object({
    // `.nullish()` (not `.optional()`) — Podcast Index returns explicit `null`
    // for absent fields (e.g. feedItunesId, images), which optional() rejects.
    // mapEpisode already treats null as absent via `??`/truthy checks.
    title: z.string().nullish(),
    feedTitle: z.string().nullish(),
    feedUrl: z.string().nullish(),
    feedItunesId: z.number().nullish(),
    guid: z.string().nullish(),
    enclosureUrl: z.string().nullish(),
    link: z.string().nullish(),
    datePublished: z.number().nullish(),
    duration: z.number().nullish(),
    description: z.string().nullish(),
    image: z.string().nullish(),
    feedImage: z.string().nullish(),
  })
  .passthrough();

export type RawPodcastIndexEpisode = z.infer<typeof rawEpisodeSchema>;

const searchByPersonResponseSchema = z
  .object({
    status: z.unknown(),
    // Coerce a null/absent items to [] — PI returns explicit null on some
    // error/no-result responses, which a plain optional().default() rejects.
    items: z.preprocess((value) => value ?? [], z.array(rawEpisodeSchema)),
    count: z.unknown(),
  })
  .passthrough();

/**
 * Maps a raw search result to our episode shape. Returns undefined (the skip
 * signal — callers filter these out) when a field we can't do without is
 * missing: feedUrl, enclosureUrl, datePublished, or guid. `guid` is required
 * because it forms the episode identity (`{showId}#{guid}`); without it two
 * distinct guid-less episodes of a show would collide on one id — and that id
 * is the permanent-exclusion key, so a delivery would blackhole the rest.
 */
export function mapEpisode(
  raw: RawPodcastIndexEpisode,
): PodcastIndexEpisode | undefined {
  if (!raw.feedUrl || !raw.enclosureUrl || !raw.datePublished || !raw.guid) {
    return undefined;
  }

  const artwork = raw.image ?? raw.feedImage ?? undefined;
  return {
    title: raw.title ?? "",
    feedTitle: raw.feedTitle ?? "",
    feedUrl: raw.feedUrl,
    ...(raw.feedItunesId ? { feedItunesId: raw.feedItunesId } : {}),
    guid: raw.guid,
    enclosureUrl: raw.enclosureUrl,
    ...(raw.link ? { episodeUrl: raw.link } : {}),
    publishedAt: raw.datePublished * 1000,
    ...(raw.duration ? { durationMinutes: Math.round(raw.duration / 60) } : {}),
    description: raw.description ?? "",
    ...(artwork ? { artworkUrl: artwork } : {}),
  };
}

export interface PodcastIndexClient {
  searchByPerson(name: string): Promise<PodcastIndexEpisode[]>;
}

class PodcastIndexApiClient implements PodcastIndexClient {
  // One process-wide queue so every request (present and future methods)
  // stays under Podcast Index's rate limits regardless of caller fan-out.
  private readonly queue = new PQueue({
    concurrency: MAX_CONCURRENT_REQUESTS,
    interval: RATE_INTERVAL_MS,
    intervalCap: MAX_REQUESTS_PER_INTERVAL,
  });

  public constructor(
    private readonly credentials: PodcastIndexCredentials,
    private readonly logger: Logger,
  ) {}

  public async searchByPerson(name: string): Promise<PodcastIndexEpisode[]> {
    const response = await this.queue.add(() =>
      got
        .get(`${BASE_URL}/search/byperson`, {
          searchParams: { q: name, max: DEFAULT_MAX_RESULTS },
          headers: podcastIndexAuthHeaders(this.credentials),
          retry: { limit: 2 },
          timeout: { request: 15_000 },
        })
        .json<unknown>(),
    );

    const parsed = searchByPersonResponseSchema.parse(response);
    const episodes: PodcastIndexEpisode[] = [];
    for (const raw of parsed.items) {
      const episode = mapEpisode(raw);
      if (episode) {
        episodes.push(episode);
      } else {
        this.logger.debug(`Skipping Podcast Index episode missing required fields`, {
          title: raw.title,
        });
      }
    }
    return episodes;
  }
}

/** Returns the configured Podcast Index client, or null when no credentials are set. */
export function createPodcastIndexClient(logger: Logger): PodcastIndexClient | null {
  const { PODCASTINDEX_KEY: key, PODCASTINDEX_SECRET: secret } = config;
  if (!key && !secret) return null;
  if (!key || !secret) {
    logger.warn("Podcast Index requires both PODCASTINDEX_KEY and PODCASTINDEX_SECRET");
    return null;
  }
  return new PodcastIndexApiClient({ key, secret }, logger);
}
