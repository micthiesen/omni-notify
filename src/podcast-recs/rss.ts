import got from "got";
import { decode } from "html-entities";
import { DOMParser } from "linkedom";

const DEFAULT_MAX_EPISODES = 30;
const DESCRIPTION_MAX_CHARS = 500;
const REQUEST_TIMEOUT_MS = 15_000;
const USER_AGENT = "omni-notify/1.0";

/** One episode parsed from a podcast's RSS feed. */
export interface FeedEpisode {
  guid: string;
  title: string;
  publishedAt: number;
  durationMinutes?: number;
  description: string;
  link?: string;
  /**
   * Enclosure (audio) URL. The most reliable cross-system episode key: RSS
   * `<guid>` is frequently rewritten by hosting platforms (Simplecast,
   * Megaphone) and will not match a podcast client's stored guid, but the
   * enclosure URL is shared. Used to match episodes against Castro.
   */
  enclosureUrl?: string;
}

/**
 * Normalizes a title for loose matching: lowercase, strip punctuation and
 * diacritics, collapse whitespace. Shared by itunes.ts (show matching) and
 * findEpisodeByTitle (episode matching).
 */
export function normalizeTitle(title: string): string {
  return title
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "") // strip diacritics (combining marks after NFD)
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s]/gu, " ") // strip punctuation
    .replace(/\s+/g, " ")
    .trim();
}

/** Fetches a podcast's RSS feed and parses its episodes. Errors propagate to the caller. */
export async function fetchFeedEpisodes(
  feedUrl: string,
  options: { maxEpisodes?: number } = {},
): Promise<FeedEpisode[]> {
  const xml = await got(feedUrl, {
    timeout: { request: REQUEST_TIMEOUT_MS },
    headers: { "User-Agent": USER_AGENT },
  }).text();
  return parseFeedEpisodes(xml, options.maxEpisodes);
}

/**
 * Parses RSS 2.0 `<item>` elements into FeedEpisodes. Pure — no network
 * access. Items missing a usable guid or a parseable pubDate are skipped.
 * Sorted newest first, capped at maxEpisodes.
 */
export function parseFeedEpisodes(
  xml: string,
  maxEpisodes = DEFAULT_MAX_EPISODES,
): FeedEpisode[] {
  const doc = new DOMParser().parseFromString(xml, "text/xml");
  const items = Array.from(doc.getElementsByTagName("item"));

  const episodes: FeedEpisode[] = [];
  for (const item of items) {
    const guid = extractGuid(item);
    if (!guid) continue;

    const rawTitle = firstText(item, "title") ?? "";
    const title = decode(stripCData(rawTitle)).trim();

    const publishedAt = parsePubDate(firstText(item, "pubDate"));
    if (publishedAt === undefined) continue;

    const durationMinutes = parseDurationMinutes(firstText(item, "itunes:duration"));

    const rawDescription =
      firstText(item, "description") ?? firstText(item, "itunes:summary") ?? "";
    const description = cleanDescription(rawDescription);

    const link = firstText(item, "link")?.trim() || undefined;

    const enclosureUrl =
      item.getElementsByTagName("enclosure")[0]?.getAttribute("url")?.trim() ||
      undefined;

    episodes.push({
      guid,
      title,
      publishedAt,
      durationMinutes,
      description,
      link,
      enclosureUrl,
    });
  }

  episodes.sort((a, b) => b.publishedAt - a.publishedAt);
  return episodes.slice(0, maxEpisodes);
}

/**
 * Finds an episode by title using the same loose normalization as
 * pickBestShowMatch: exact normalized match wins, else the longest
 * containment match (either direction), else undefined.
 */
export function findEpisodeByTitle(
  episodes: FeedEpisode[],
  episodeTitle: string,
): FeedEpisode | undefined {
  const target = normalizeTitle(episodeTitle);
  if (!target) return undefined;

  const exact = episodes.find((episode) => normalizeTitle(episode.title) === target);
  if (exact) return exact;

  let best: FeedEpisode | undefined;
  let bestLength = -1;
  for (const episode of episodes) {
    const normalized = normalizeTitle(episode.title);
    if (!normalized) continue;
    if (normalized.includes(target) || target.includes(normalized)) {
      if (normalized.length > bestLength) {
        best = episode;
        bestLength = normalized.length;
      }
    }
  }
  return best;
}

function firstText(item: Element, tagName: string): string | undefined {
  const el = item.getElementsByTagName(tagName)[0];
  return el?.textContent ?? undefined;
}

function extractGuid(item: Element): string | undefined {
  const guid = firstText(item, "guid")?.trim();
  if (guid) return guid;

  const enclosureUrl = item
    .getElementsByTagName("enclosure")[0]
    ?.getAttribute("url")
    ?.trim();
  if (enclosureUrl) return enclosureUrl;

  const link = firstText(item, "link")?.trim();
  if (link) return link;

  return undefined;
}

function stripCData(text: string): string {
  const match = text.match(/^\s*<!\[CDATA\[([\s\S]*)\]\]>\s*$/);
  return match ? match[1] : text;
}

function parsePubDate(rawPubDate: string | undefined): number | undefined {
  if (!rawPubDate) return undefined;
  const timestamp = Date.parse(stripCData(rawPubDate).trim());
  return Number.isNaN(timestamp) ? undefined : timestamp;
}

/** Handles plain seconds ("3720"), "MM:SS", and "HH:MM:SS". */
function parseDurationMinutes(raw: string | undefined): number | undefined {
  if (!raw) return undefined;
  const value = stripCData(raw).trim();
  if (!value) return undefined;

  if (/^\d+$/.test(value)) {
    return Math.round(Number(value) / 60);
  }

  const parts = value.split(":");
  if (parts.length < 2 || parts.length > 3 || !parts.every((p) => /^\d+$/.test(p))) {
    return undefined;
  }

  const numbers = parts.map(Number);
  const totalSeconds =
    numbers.length === 3
      ? numbers[0] * 3600 + numbers[1] * 60 + numbers[2]
      : numbers[0] * 60 + numbers[1];
  return Math.round(totalSeconds / 60);
}

function cleanDescription(rawDescription: string): string {
  const text = decode(stripCData(rawDescription))
    .replace(/<[^>]*>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return text.length > DESCRIPTION_MAX_CHARS
    ? text.slice(0, DESCRIPTION_MAX_CHARS)
    : text;
}
