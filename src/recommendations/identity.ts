import type { Logger } from "@micthiesen/mitools/logging";
import { IdentityAliasEntity } from "./persistence.js";
import { findByExternalId, searchTitles } from "./tmdb/client.js";
import type { CanonicalId, ExternalIds, MediaItem } from "./types.js";
import { type MediaType, makeCanonicalId } from "./types.js";

/** Below this confidence a resolution is treated as unresolved. */
export const RESOLUTION_CONFIDENCE_THRESHOLD = 0.8;

export interface Resolution {
  canonicalId: CanonicalId | null;
  confidence: number;
  resolutionPath: "external-id" | "tmdb-find" | "tmdb-search" | "unresolved";
}

/**
 * Extract external ids embedded in a server-native GUID. Handles the common
 * formats: plain `tmdb://123`, `imdb://tt123`, `tvdb://123` and legacy agent
 * URIs like `com.plexapp.agents.imdb://tt0111161?lang=en`.
 */
export function parseGuidExternalIds(guid: string): ExternalIds {
  const ids: ExternalIds = {};
  const match = guid.match(
    /(?:^|\.)(imdb|tmdb|themoviedb|tvdb|thetvdb):\/\/([a-zA-Z0-9]+)/,
  );
  if (!match) return ids;
  const [, source, value] = match;
  if (source === "imdb" && value.startsWith("tt")) {
    ids.imdb = value;
  } else if (source === "tmdb" || source === "themoviedb") {
    const num = Number(value);
    if (Number.isInteger(num)) ids.tmdb = num;
  } else if (source === "tvdb" || source === "thetvdb") {
    const num = Number(value);
    if (Number.isInteger(num)) ids.tvdb = num;
  }
  return ids;
}

/**
 * Resolve a media-server item to its canonical TMDB identity. Results
 * (including failures) are cached by GUID so each library item costs at most
 * one round of TMDB lookups.
 */
export async function resolveIdentity(
  item: MediaItem,
  logger: Logger,
  options: { allowNetwork?: boolean } = {},
): Promise<Resolution> {
  const allowNetwork = options.allowNetwork ?? true;
  const cached = IdentityAliasEntity.get({ guid: item.guid });
  if (cached) {
    return {
      canonicalId: cached.canonicalId as CanonicalId | null,
      confidence: cached.confidence,
      resolutionPath: cached.resolutionPath,
    };
  }

  const external = { ...parseGuidExternalIds(item.guid), ...item.externalIds };

  // Direct TMDB id: born canonical, no network needed.
  if (external.tmdb !== undefined) {
    const resolution: Resolution = {
      canonicalId: makeCanonicalId(item.mediaType, external.tmdb),
      confidence: 1,
      resolutionPath: "external-id",
    };
    cacheResolution(item, resolution);
    return resolution;
  }

  // Without network access, leave the item unresolved and UNcached so a
  // later full-resolution pass can still fill it in.
  if (!allowNetwork) {
    return { canonicalId: null, confidence: 0, resolutionPath: "unresolved" };
  }

  const resolution = await resolveViaNetwork(item, external, logger);
  cacheResolution(item, resolution);
  return resolution;
}

function cacheResolution(item: MediaItem, resolution: Resolution): void {
  IdentityAliasEntity.upsert({
    guid: item.guid,
    canonicalId: resolution.canonicalId,
    confidence: resolution.confidence,
    resolutionPath: resolution.resolutionPath,
    title: item.title,
    resolvedAt: Date.now(),
  });
}

async function resolveViaNetwork(
  item: MediaItem,
  external: ExternalIds,
  logger: Logger,
): Promise<Resolution> {
  // IMDb/TVDB id via TMDB /find.
  const findSource = external.imdb
    ? ({ id: external.imdb, source: "imdb_id" } as const)
    : external.tvdb !== undefined
      ? ({ id: String(external.tvdb), source: "tvdb_id" } as const)
      : undefined;
  if (findSource) {
    try {
      const matches = (await findByExternalId(findSource.id, findSource.source)).filter(
        (t) => t.mediaType === item.mediaType,
      );
      if (matches.length === 1) {
        return {
          canonicalId: makeCanonicalId(item.mediaType, matches[0].tmdbId),
          confidence: 0.98,
          resolutionPath: "tmdb-find",
        };
      }
    } catch (error) {
      logger.warn(
        `TMDB find failed for "${item.title}" (${findSource.source}=${findSource.id})`,
        (error as Error).message,
      );
    }
  }

  // Last resort: text search constrained by title/year.
  try {
    const results = await searchTitles(item.title, item.mediaType, item.year);
    const scored = scoreSearchResults(item, results);
    if (scored) return scored;
  } catch (error) {
    logger.warn(`TMDB search failed for "${item.title}"`, (error as Error).message);
  }

  return { canonicalId: null, confidence: 0, resolutionPath: "unresolved" };
}

export function scoreSearchResults(
  item: { title: string; year?: number; mediaType: MediaType },
  results: { tmdbId: number; title: string; year?: number; voteCount: number }[],
): Resolution | undefined {
  const normalizedTarget = normalizeTitle(item.title);
  const titleMatches = results.filter(
    (r) => normalizeTitle(r.title) === normalizedTarget,
  );
  const yearMatches = item.year
    ? titleMatches.filter(
        (r) => r.year !== undefined && Math.abs(r.year - item.year!) <= 1,
      )
    : titleMatches;

  if (yearMatches.length === 1) {
    return {
      canonicalId: makeCanonicalId(item.mediaType, yearMatches[0].tmdbId),
      confidence: item.year ? 0.95 : 0.85,
      resolutionPath: "tmdb-search",
    };
  }
  if (yearMatches.length > 1) {
    // Multiple survivors: only accept a clearly dominant match.
    const sorted = [...yearMatches].sort((a, b) => b.voteCount - a.voteCount);
    if (sorted[0].voteCount >= 10 * Math.max(sorted[1].voteCount, 1)) {
      return {
        canonicalId: makeCanonicalId(item.mediaType, sorted[0].tmdbId),
        confidence: 0.85,
        resolutionPath: "tmdb-search",
      };
    }
    return { canonicalId: null, confidence: 0.5, resolutionPath: "unresolved" };
  }
  return undefined;
}

function normalizeTitle(title: string): string {
  return title
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}
