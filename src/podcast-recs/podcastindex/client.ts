import type { Logger } from "@micthiesen/mitools/logging";
import { Data, Clock, Effect, Ref, Schedule, Schema } from "effect";
import { fetchPublicText, PUBLIC_HTTP_USER_AGENT } from "../../effect/publicHttp.js";
import config from "../../utils/config.js";
import { type PodcastIndexCredentials, podcastIndexAuthHeaders } from "./auth.js";
import type { PodcastIndexEpisode } from "./types.js";

const BASE_URL = "https://api.podcastindex.org/api/1.0";
const DEFAULT_MAX_RESULTS = 20;
const PODCAST_INDEX_RESPONSE_MAX_BYTES = 2 * 1024 * 1024;

const MAX_CONCURRENT_REQUESTS = 4;
const MAX_REQUESTS_PER_INTERVAL = 6;
const RATE_INTERVAL_MS = 1000;

// Tolerate whatever extra fields the API adds/renames over time — we only
// pull the handful of fields we care about, and every one of them is
// optional here so a missing/renamed field degrades to a skip rather than a
// thrown parse error.
const rawEpisodeSchema = Schema.Struct({
  // `.nullish()` (not `.optional()`) — Podcast Index returns explicit `null`
  // for absent fields (e.g. feedItunesId, images), which optional() rejects.
  // mapEpisode already treats null as absent via `??`/truthy checks.
  title: Schema.optional(Schema.NullOr(Schema.String)),
  feedTitle: Schema.optional(Schema.NullOr(Schema.String)),
  feedUrl: Schema.optional(Schema.NullOr(Schema.String)),
  feedItunesId: Schema.optional(Schema.NullOr(Schema.Number)),
  guid: Schema.optional(Schema.NullOr(Schema.String)),
  enclosureUrl: Schema.optional(Schema.NullOr(Schema.String)),
  link: Schema.optional(Schema.NullOr(Schema.String)),
  datePublished: Schema.optional(Schema.NullOr(Schema.Number)),
  duration: Schema.optional(Schema.NullOr(Schema.Number)),
  description: Schema.optional(Schema.NullOr(Schema.String)),
  image: Schema.optional(Schema.NullOr(Schema.String)),
  feedImage: Schema.optional(Schema.NullOr(Schema.String)),
});

export type RawPodcastIndexEpisode = Schema.Schema.Type<typeof rawEpisodeSchema>;

const searchByPersonResponseSchema = Schema.Struct({
  // Coerce a null/absent items to [] — PI returns explicit null on some
  // error/no-result responses, which a plain optional().default() rejects.
  items: Schema.optional(Schema.NullOr(Schema.Array(rawEpisodeSchema))),
});

class PodcastIndexRequestError extends Data.TaggedError("PodcastIndexRequestError")<{
  readonly name: string;
  readonly cause: unknown;
}> {}

interface RequestControl {
  readonly apply: <A, E, R>(effect: Effect.Effect<A, E, R>) => Effect.Effect<A, E, R>;
}

const requestControl = Effect.runSync(
  Effect.gen(function* () {
    const semaphore = yield* Effect.makeSemaphore(MAX_CONCURRENT_REQUESTS);
    const rateState = yield* Ref.make({ windowStart: 0, used: 0 });
    const takeRatePermit: Effect.Effect<void> = Effect.suspend(() =>
      Effect.flatMap(Clock.currentTimeMillis, (now) =>
        Ref.modify(rateState, (state) => {
          if (now - state.windowStart >= RATE_INTERVAL_MS) {
            return [0, { windowStart: now, used: 1 }] as const;
          }
          if (state.used < MAX_REQUESTS_PER_INTERVAL) {
            return [0, { ...state, used: state.used + 1 }] as const;
          }
          return [state.windowStart + RATE_INTERVAL_MS - now, state] as const;
        }),
      ).pipe(
        Effect.flatMap((waitMs) =>
          waitMs > 0
            ? Effect.sleep(`${waitMs} millis`).pipe(Effect.zipRight(takeRatePermit))
            : Effect.void,
        ),
      ),
    );
    return {
      apply: (effect) =>
        takeRatePermit.pipe(Effect.zipRight(semaphore.withPermits(1)(effect))),
    } satisfies RequestControl;
  }),
);

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
  searchByPerson(name: string): Effect.Effect<PodcastIndexEpisode[], unknown>;
}

class PodcastIndexApiClient implements PodcastIndexClient {
  public constructor(
    private readonly credentials: PodcastIndexCredentials,
    private readonly logger: Logger,
  ) {}

  public searchByPerson(name: string): Effect.Effect<PodcastIndexEpisode[], unknown> {
    return this.searchByPersonEffect(name);
  }

  private searchByPersonEffect(name: string) {
    return Effect.gen(this, function* () {
      const attempt = requestControl.apply(
        fetchPublicText(
          `${BASE_URL}/search/byperson`,
          {
            searchParams: { q: name, max: DEFAULT_MAX_RESULTS },
            headers: {
              ...podcastIndexAuthHeaders(this.credentials),
              "User-Agent": PUBLIC_HTTP_USER_AGENT,
            },
            retry: { limit: 0 },
            timeout: { request: 15_000 },
          },
          `Podcast Index byperson request failed for ${name}`,
          undefined,
          PODCAST_INDEX_RESPONSE_MAX_BYTES,
        ).pipe(
          Effect.mapError((cause) => new PodcastIndexRequestError({ name, cause })),
        ),
      );
      const responseText = yield* attempt.pipe(
        Effect.retry(
          Schedule.exponential("200 millis").pipe(Schedule.compose(Schedule.recurs(2))),
        ),
      );

      const parsed = yield* Schema.decodeUnknown(
        Schema.parseJson(searchByPersonResponseSchema),
      )(responseText);
      const episodes: PodcastIndexEpisode[] = [];
      for (const raw of parsed.items ?? []) {
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
    });
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
