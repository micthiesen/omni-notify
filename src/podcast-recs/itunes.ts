import { Data, Effect, Schema } from "effect";
import {
  fetchPublicText,
  PUBLIC_HTTP_USER_AGENT,
  type PublicTextRequest,
} from "../effect/publicHttp.js";
import { normalizeTitle } from "./titles.js";

const SEARCH_URL = "https://itunes.apple.com/search";
const DEFAULT_LIMIT = 5;
const ITUNES_RESPONSE_MAX_BYTES = 1024 * 1024;

/** A podcast show as returned by the iTunes Search API. */
export interface ItunesShow {
  itunesId: number;
  title: string;
  feedUrl?: string;
  artworkUrl?: string;
  genres: string[];
}

const itunesResultSchema = Schema.Struct({
  collectionId: Schema.optional(Schema.Number),
  collectionName: Schema.optional(Schema.String),
  feedUrl: Schema.optional(Schema.String),
  artworkUrl600: Schema.optional(Schema.String),
  artworkUrl100: Schema.optional(Schema.String),
  genres: Schema.optional(Schema.Array(Schema.String)),
});

const itunesSearchResponseSchema = Schema.Struct({
  results: Schema.optionalWith(Schema.Array(itunesResultSchema), {
    default: () => [],
  }),
});

class ItunesRequestError extends Data.TaggedError("ItunesRequestError")<{
  readonly term: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `iTunes search failed for ${this.term}: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

/** Searches the (keyless) iTunes Search API for podcast shows matching `term`. */
export function searchItunesPodcastsEffect(
  term: string,
  limit = DEFAULT_LIMIT,
  dependencies: {
    readonly request?: PublicTextRequest;
    readonly maxResponseBytes?: number;
  } = {},
) {
  return Effect.gen(function* () {
    const responseText = yield* fetchPublicText(
      SEARCH_URL,
      {
        searchParams: { media: "podcast", entity: "podcast", term, limit },
        headers: { "User-Agent": PUBLIC_HTTP_USER_AGENT },
        timeout: { request: 15_000 },
      },
      `iTunes search request failed for ${term}`,
      dependencies.request,
      dependencies.maxResponseBytes ?? ITUNES_RESPONSE_MAX_BYTES,
    ).pipe(Effect.mapError((cause) => new ItunesRequestError({ term, cause })));

    const parsed = yield* Schema.decodeUnknown(
      Schema.parseJson(itunesSearchResponseSchema),
    )(responseText).pipe(
      Effect.mapError((cause) => new ItunesRequestError({ term, cause })),
    );

    const shows: ItunesShow[] = [];
    for (const result of parsed.results) {
      if (!result.collectionId || !result.collectionName) continue;
      shows.push({
        itunesId: result.collectionId,
        title: result.collectionName,
        feedUrl: result.feedUrl,
        artworkUrl: result.artworkUrl600 ?? result.artworkUrl100,
        genres: [...(result.genres ?? [])],
      });
    }
    return shows;
  });
}

/**
 * Picks the show whose title best matches `showTitle`, using loose
 * normalized comparison. Pure — no network access. Exact normalized match
 * wins; else a prefix/containment match (either direction); else undefined.
 */
export function pickBestShowMatch(
  shows: ItunesShow[],
  showTitle: string,
): ItunesShow | undefined {
  const target = normalizeTitle(showTitle);
  if (!target) return undefined;

  const exact = shows.find((show) => normalizeTitle(show.title) === target);
  if (exact) return exact;

  return shows.find((show) => {
    const normalized = normalizeTitle(show.title);
    return normalized && (normalized.includes(target) || target.includes(normalized));
  });
}
