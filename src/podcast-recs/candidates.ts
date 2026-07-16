import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import PQueue from "p-queue";
import type { PodcastAccountClient } from "./account.js";
import { normalizeTitle } from "./filters.js";
import { pickBestShowMatch, searchItunesPodcasts } from "./itunes.js";
import type { PodcastIndexEpisode } from "./podcastindex/types.js";
import { fetchFeedEpisodes, findEpisodeByTitle } from "./rss.js";
import type { DiscoveredEpisode, EpisodeCandidate } from "./types.js";
import { makeEpisodeId, makeShowId } from "./types.js";

const RESOLVE_CONCURRENCY = 3;

/** A show resolved to a feed URL and identity, ready for RSS episode lookup. */
interface ResolvedShow {
  title: string;
  feedUrl: string;
  itunesId?: number;
  artworkUrl?: string;
  genres: string[];
}

/**
 * Resolve discovered episodes into verified candidates: show identity via the
 * iTunes Search API (falling back to Castro's podcast search when iTunes can't
 * place the show), then the episode itself (and its authoritative release
 * date) from the show's actual RSS feed. Anything that cannot be verified is
 * dropped with a logged reason — an unverifiable candidate is never
 * recommendable, which is the deterministic version of the old briefing's
 * "open the episode page before recommending" rule.
 */
export async function resolveCandidates(
  discovered: DiscoveredEpisode[],
  account: PodcastAccountClient | undefined,
  logger: Logger,
  logFile?: LogFile,
): Promise<EpisodeCandidate[]> {
  const queue = new PQueue({ concurrency: RESOLVE_CONCURRENCY });
  const dropped: string[] = [];

  const resolved = await Promise.all(
    discovered.map((item) =>
      queue.add(async () => {
        try {
          return await resolveOne(item, account);
        } catch (error) {
          dropped.push(
            `- ${item.showTitle} — ${item.episodeTitle}: ${(error as Error).message}`,
          );
          return undefined;
        }
      }),
    ),
  );

  const seen = new Set<string>();
  const candidates: EpisodeCandidate[] = [];
  for (const candidate of resolved) {
    if (!candidate || seen.has(candidate.episodeId)) continue;
    seen.add(candidate.episodeId);
    candidates.push(candidate);
  }

  logger.info(`Resolved ${candidates.length}/${discovered.length} discovered episodes`);
  if (dropped.length > 0) {
    logFile?.section("Resolution Failures", dropped.join("\n"));
  }
  return candidates;
}

async function resolveOne(
  item: DiscoveredEpisode,
  account: PodcastAccountClient | undefined,
): Promise<EpisodeCandidate> {
  const show = await resolveShow(item.showTitle, account);
  if (!show) throw new Error("show not found on iTunes or Castro");

  const episodes = await fetchFeedEpisodes(show.feedUrl, { maxEpisodes: 30 });
  const episode = findEpisodeByTitle(episodes, item.episodeTitle);
  if (!episode) throw new Error("episode not found in RSS feed");

  const showId = makeShowId({ itunesId: show.itunesId, feedUrl: show.feedUrl });
  if (!showId) throw new Error("could not build canonical show id");

  return {
    episodeId: makeEpisodeId(showId, episode.guid),
    showId,
    showTitle: show.title,
    episodeTitle: episode.title,
    feedUrl: show.feedUrl,
    itunesId: show.itunesId,
    artworkUrl: show.artworkUrl,
    episodeGuid: episode.guid,
    mediaUrl: episode.enclosureUrl,
    episodeUrl: episode.link,
    publishedAt: episode.publishedAt,
    durationMinutes: episode.durationMinutes,
    description: episode.description,
    showGenres: show.genres,
    discoveredVia: item.context,
    sourceUrl: item.sourceUrl,
    matchedVoices: item.matchedVoices,
  };
}

/**
 * Place a discovered show on a feed URL. iTunes is the primary index; Castro's
 * podcast search is a fallback that catches private feeds and niche shows
 * iTunes misses (Castro-resolved shows carry no genre metadata).
 */
async function resolveShow(
  showTitle: string,
  account: PodcastAccountClient | undefined,
): Promise<ResolvedShow | undefined> {
  const itunes = pickBestShowMatch(await searchItunesPodcasts(showTitle), showTitle);
  if (itunes?.feedUrl) {
    return {
      title: itunes.title,
      feedUrl: itunes.feedUrl,
      itunesId: itunes.itunesId,
      artworkUrl: itunes.artworkUrl,
      genres: itunes.genres,
    };
  }

  if (!account) return undefined;
  const result = await account.searchPodcasts(showTitle);
  if (result.status !== "ok") return undefined;
  const match = pickBestByTitle(result.value, showTitle);
  if (!match?.feedUrl) return undefined;
  return {
    title: match.title,
    feedUrl: match.feedUrl,
    itunesId: match.itunesId,
    artworkUrl: match.artworkUrl,
    genres: [],
  };
}

/**
 * A Podcast Index episode is already fully resolved (feed URL, guid, enclosure,
 * verified publish date), so it maps straight to a candidate — no iTunes/RSS
 * round-trip needed. `voice` is the followed person whose byperson search
 * surfaced it.
 */
export function podcastIndexToCandidate(
  episode: PodcastIndexEpisode,
  voice: string,
): EpisodeCandidate | undefined {
  const showId = makeShowId({
    itunesId: episode.feedItunesId,
    feedUrl: episode.feedUrl,
  });
  if (!showId) return undefined;
  return {
    episodeId: makeEpisodeId(showId, episode.guid),
    showId,
    showTitle: episode.feedTitle,
    episodeTitle: episode.title,
    feedUrl: episode.feedUrl,
    itunesId: episode.feedItunesId,
    artworkUrl: episode.artworkUrl,
    episodeGuid: episode.guid,
    mediaUrl: episode.enclosureUrl,
    episodeUrl: episode.episodeUrl,
    publishedAt: episode.publishedAt,
    durationMinutes: episode.durationMinutes,
    description: episode.description,
    showGenres: [],
    discoveredVia: `guest: ${voice} (Podcast Index)`,
    matchedVoices: [voice],
  };
}

/** Loose title match shared by the Castro fallback: exact, then containment. */
export function pickBestByTitle<T extends { title: string }>(
  items: T[],
  showTitle: string,
): T | undefined {
  const target = normalizeTitle(showTitle);
  const exact = items.find((item) => normalizeTitle(item.title) === target);
  if (exact) return exact;
  return items.find((item) => {
    const candidate = normalizeTitle(item.title);
    return candidate.includes(target) || target.includes(candidate);
  });
}
