import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import PQueue from "p-queue";
import { pickBestShowMatch, searchItunesPodcasts } from "./itunes.js";
import { fetchFeedEpisodes, findEpisodeByTitle } from "./rss.js";
import type { DiscoveredEpisode, EpisodeCandidate } from "./types.js";
import { makeEpisodeId, makeShowId } from "./types.js";

const RESOLVE_CONCURRENCY = 3;

/**
 * Resolve discovered episodes into verified candidates: show identity via the
 * iTunes Search API, then the episode itself (and its authoritative release
 * date) from the show's actual RSS feed. Anything that cannot be verified is
 * dropped with a logged reason — an unverifiable candidate is never
 * recommendable, which is the deterministic version of the old briefing's
 * "open the episode page before recommending" rule.
 */
export async function resolveCandidates(
  discovered: DiscoveredEpisode[],
  logger: Logger,
  logFile?: LogFile,
): Promise<EpisodeCandidate[]> {
  const queue = new PQueue({ concurrency: RESOLVE_CONCURRENCY });
  const dropped: string[] = [];

  const resolved = await Promise.all(
    discovered.map((item) =>
      queue.add(async () => {
        try {
          return await resolveOne(item);
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

async function resolveOne(item: DiscoveredEpisode): Promise<EpisodeCandidate> {
  const shows = await searchItunesPodcasts(item.showTitle);
  const show = pickBestShowMatch(shows, item.showTitle);
  if (!show) throw new Error("show not found on iTunes");
  if (!show.feedUrl) throw new Error("iTunes result has no feed URL");

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
    episodeUrl: episode.link,
    publishedAt: episode.publishedAt,
    durationMinutes: episode.durationMinutes,
    description: episode.description,
    showGenres: show.genres,
    discoveredVia: item.context,
    sourceUrl: item.sourceUrl,
  };
}
