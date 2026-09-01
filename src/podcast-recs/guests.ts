import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { generateText, Output } from "ai";
import { Clock, Data, Effect } from "effect";
import { z } from "zod";
import { getRecsShortlistModel } from "../ai/registry.js";
import { searchWebEffect } from "../ai/tools/webSearch.js";
import type { PodcastAccountClient } from "./account.js";
import { podcastIndexToCandidate, resolveCandidatesEffect } from "./candidates.js";
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
class GuestDiscoveryError extends Data.TaggedError("GuestDiscoveryError")<{
  readonly voice: string;
  readonly cause: unknown;
}> {}

export function discoverGuestAppearancesEffect(
  voices: string[],
  account: PodcastAccountClient | undefined,
  logger: Logger,
  logFile?: LogFile,
): Effect.Effect<EpisodeCandidate[], GuestDiscoveryError> {
  return Effect.gen(function* () {
    if (voices.length === 0) return [];
    const pi = createPodcastIndexClient(logger);
    const now = yield* Clock.currentTimeMillis;
    const cutoff = now - RECENT_EPISODE_WINDOW_MS;
    const perVoice = yield* Effect.forEach(
      voices,
      (voice) => discoverForVoiceEffect(voice, pi, account, cutoff, logger),
      { concurrency: VOICE_CONCURRENCY },
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
  });
}

function discoverForVoiceEffect(
  voice: string,
  pi: PodcastIndexClient | null,
  account: PodcastAccountClient | undefined,
  cutoff: number,
  logger: Logger,
): Effect.Effect<EpisodeCandidate[], GuestDiscoveryError> {
  return Effect.gen(function* () {
    const [fromPi, fromTavily] = yield* Effect.all(
      [
        discoverViaPodcastIndexEffect(voice, pi, cutoff, logger),
        discoverViaTavilyEffect(voice, account, logger),
      ],
      { concurrency: 2 },
    );
    if (fromPi === undefined && fromTavily === undefined) {
      return yield* Effect.fail(
        new GuestDiscoveryError({
          voice,
          cause: new Error("all configured guest-discovery sources failed"),
        }),
      );
    }
    return [...(fromPi ?? []), ...(fromTavily ?? [])];
  });
}

function discoverViaPodcastIndexEffect(
  voice: string,
  pi: PodcastIndexClient | null,
  cutoff: number,
  logger: Logger,
): Effect.Effect<EpisodeCandidate[] | undefined> {
  if (!pi) return Effect.succeed(undefined);
  return pi.searchByPerson(voice).pipe(
    Effect.map((episodes) =>
      episodes
        .filter((episode) => episode.publishedAt >= cutoff)
        .map((episode) => podcastIndexToCandidate(episode, voice))
        .filter((candidate): candidate is EpisodeCandidate => candidate !== undefined),
    ),
    Effect.catch((error) => {
      logger.warn(`Podcast Index byperson failed for ${voice}`, String(error));
      return Effect.succeed(undefined);
    }),
  );
}

function discoverViaTavilyEffect(
  voice: string,
  account: PodcastAccountClient | undefined,
  logger: Logger,
): Effect.Effect<EpisodeCandidate[] | undefined> {
  return Effect.gen(function* () {
    const response = yield* searchWebEffect({
      query: `"${voice}" podcast guest interview`,
      topic: "news",
      timeRange: "week",
      maxResults: 6,
      maxContentChars: 700,
    }).pipe(
      Effect.catch((error) => {
        logger.warn(`Tavily person-search failed for ${voice}`, String(error));
        return Effect.succeed(undefined);
      }),
    );
    if (!response) return undefined;
    if (response.results.length === 0) return [];

    const { model } = getRecsShortlistModel("extract-guest-appearances");
    const prompt = `Recent web results for podcast episodes possibly featuring ${voice} as a guest. Extract ONLY episodes where ${voice} is actually a guest or participant (not merely mentioned or the topic). Give the podcast show name and episode title as precisely as you can.

RESULTS:
${response.results
  .map((r) => `- ${r.title} (${r.url})\n  ${r.content.replace(/\s+/g, " ")}`)
  .join("\n")}

Return JSON only; empty array if none clearly qualify.`;

    const result = yield* Effect.tryPromise(() =>
      generateText({
        model,
        output: Output.object({ schema: extractionSchema }),
        prompt,
      }),
    ).pipe(
      Effect.catch((error) => {
        logger.warn(`Guest extraction failed for ${voice}`, String(error));
        return Effect.succeed(undefined);
      }),
    );

    if (!result) return undefined;

    const discovered: DiscoveredEpisode[] = (result.output?.episodes ?? []).map(
      (episode) => ({
        showTitle: episode.show_title,
        episodeTitle: episode.episode_title,
        context: `guest: ${voice} (web)`,
        sourceUrl: episode.source_url ?? undefined,
        matchedVoices: [voice],
      }),
    );
    if (discovered.length === 0) return [];
    return yield* resolveCandidatesEffect(discovered, account, logger);
  });
}
