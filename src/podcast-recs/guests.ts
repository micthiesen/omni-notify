import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import PQueue from "p-queue";
import { z } from "zod";
import { getRecsShortlistModel } from "../ai/registry.js";
import { searchWeb } from "../ai/tools/webSearch.js";
import type { PodcastAccountClient } from "./account.js";
import { podcastIndexToCandidate, resolveCandidates } from "./candidates.js";
import { RECENT_EPISODE_WINDOW_MS } from "./filters.js";
import {
  createPodcastIndexClient,
  type PodcastIndexClient,
} from "./podcastindex/client.js";
import type { DiscoveredEpisode, EpisodeCandidate } from "./types.js";

const VOICE_CONCURRENCY = 3;

const extractionSchema = z.object({
  episodes: z.array(
    z.object({
      show_title: z.string(),
      episode_title: z.string(),
      source_url: z.string().nullable(),
    }),
  ),
});

/**
 * Tier-1 discovery: recent episodes where one of the followed voices appears as
 * a guest. Runs BOTH sources per voice and unions them: Podcast Index
 * `byperson` (free, structured RSS person tags) and a Tavily person-search
 * (covers non-podcasters and untagged feeds). Both are needed because many
 * followed voices HOST a subscribed show — PI then returns that own show and,
 * if we treated any PI hit as "found", we'd never web-search for the guest
 * spots elsewhere that are the whole point. Tavily volume stays bounded by the
 * per-run voice rotation.
 *
 * `voices` is expected to already be the rotated per-run batch.
 */
export async function discoverGuestAppearances(
  voices: string[],
  account: PodcastAccountClient | undefined,
  logger: Logger,
  logFile?: LogFile,
): Promise<EpisodeCandidate[]> {
  if (voices.length === 0) return [];
  const pi = createPodcastIndexClient(logger);
  const cutoff = Date.now() - RECENT_EPISODE_WINDOW_MS;
  const queue = new PQueue({ concurrency: VOICE_CONCURRENCY });

  const perVoice = await Promise.all(
    voices.map((voice) =>
      queue.add(() => discoverForVoice(voice, pi, account, cutoff, logger)),
    ),
  );

  // Dedup by episode; one episode can feature several followed voices.
  const byId = new Map<string, EpisodeCandidate>();
  for (const candidate of perVoice.flatMap((r) => r ?? [])) {
    const existing = byId.get(candidate.episodeId);
    if (existing) {
      existing.matchedVoices = [
        ...new Set([
          ...(existing.matchedVoices ?? []),
          ...(candidate.matchedVoices ?? []),
        ]),
      ];
    } else {
      byId.set(candidate.episodeId, candidate);
    }
  }

  const candidates = [...byId.values()];
  logger.info(
    `Guest discovery: ${candidates.length} candidate(s) across ${voices.length} voice(s)`,
  );
  logFile?.section(
    "Guest Appearances",
    candidates
      .map(
        (c) =>
          `- ${c.showTitle} — ${c.episodeTitle} [${(c.matchedVoices ?? []).join(", ")}]`,
      )
      .join("\n") || "none",
  );
  return candidates;
}

async function discoverForVoice(
  voice: string,
  pi: PodcastIndexClient | null,
  account: PodcastAccountClient | undefined,
  cutoff: number,
  logger: Logger,
): Promise<EpisodeCandidate[]> {
  const [fromPi, fromTavily] = await Promise.all([
    discoverViaPodcastIndex(voice, pi, cutoff, logger),
    discoverViaTavily(voice, account, logger),
  ]);
  return [...fromPi, ...fromTavily];
}

async function discoverViaPodcastIndex(
  voice: string,
  pi: PodcastIndexClient | null,
  cutoff: number,
  logger: Logger,
): Promise<EpisodeCandidate[]> {
  if (!pi) return [];
  try {
    const episodes = await pi.searchByPerson(voice);
    return episodes
      .filter((episode) => episode.publishedAt >= cutoff)
      .map((episode) => podcastIndexToCandidate(episode, voice))
      .filter((candidate): candidate is EpisodeCandidate => candidate !== undefined);
  } catch (error) {
    logger.warn(`Podcast Index byperson failed for ${voice}`, (error as Error).message);
    return [];
  }
}

async function discoverViaTavily(
  voice: string,
  account: PodcastAccountClient | undefined,
  logger: Logger,
): Promise<EpisodeCandidate[]> {
  const response = await searchWeb({
    query: `"${voice}" podcast guest interview`,
    topic: "news",
    timeRange: "week",
    maxResults: 6,
    maxContentChars: 700,
  }).catch((error) => {
    logger.warn(`Tavily person-search failed for ${voice}`, (error as Error).message);
    return { results: [] };
  });
  if (response.results.length === 0) return [];

  const { model } = getRecsShortlistModel("extract-guest-appearances");
  const prompt = `Recent web results for podcast episodes possibly featuring ${voice} as a guest. Extract ONLY episodes where ${voice} is actually a guest or participant (not merely mentioned or the topic). Give the podcast show name and episode title as precisely as you can.

RESULTS:
${response.results
  .map((r) => `- ${r.title} (${r.url})\n  ${r.content.replace(/\s+/g, " ")}`)
  .join("\n")}

Return JSON only; empty array if none clearly qualify.`;

  const result = await generateText({
    model,
    output: Output.object({ schema: extractionSchema }),
    prompt,
  }).catch((error) => {
    logger.warn(`Guest extraction failed for ${voice}`, (error as Error).message);
    return undefined;
  });

  const discovered: DiscoveredEpisode[] = (result?.output?.episodes ?? []).map(
    (episode) => ({
      showTitle: episode.show_title,
      episodeTitle: episode.episode_title,
      context: `guest: ${voice} (web)`,
      sourceUrl: episode.source_url ?? undefined,
      matchedVoices: [voice],
    }),
  );
  if (discovered.length === 0) return [];
  return resolveCandidates(discovered, account, logger);
}
