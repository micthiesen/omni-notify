import got from "got";
import { z } from "zod";
import { normalizeTitle } from "./titles.js";

const SEARCH_URL = "https://itunes.apple.com/search";
const DEFAULT_LIMIT = 5;

/** A podcast show as returned by the iTunes Search API. */
export interface ItunesShow {
  itunesId: number;
  title: string;
  feedUrl?: string;
  artworkUrl?: string;
  genres: string[];
}

const itunesResultSchema = z
  .object({
    collectionId: z.number().optional(),
    collectionName: z.string().optional(),
    feedUrl: z.string().optional(),
    artworkUrl600: z.string().optional(),
    artworkUrl100: z.string().optional(),
    genres: z.array(z.string()).optional(),
  })
  .passthrough();

const itunesSearchResponseSchema = z
  .object({
    results: z.array(itunesResultSchema).optional().default([]),
  })
  .passthrough();

/** Searches the (keyless) iTunes Search API for podcast shows matching `term`. */
export async function searchItunesPodcasts(
  term: string,
  limit = DEFAULT_LIMIT,
): Promise<ItunesShow[]> {
  const raw = await got
    .get(SEARCH_URL, {
      searchParams: { media: "podcast", entity: "podcast", term, limit },
      timeout: { request: 15_000 },
    })
    .json<unknown>();

  const parsed = itunesSearchResponseSchema.parse(raw);

  const shows: ItunesShow[] = [];
  for (const result of parsed.results) {
    if (!result.collectionId || !result.collectionName) continue;
    shows.push({
      itunesId: result.collectionId,
      title: result.collectionName,
      feedUrl: result.feedUrl,
      artworkUrl: result.artworkUrl600 ?? result.artworkUrl100,
      genres: result.genres ?? [],
    });
  }
  return shows;
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
