import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output } from "ai";
import { z } from "zod";
import { getRecsShortlistModel } from "../ai/registry.js";
import { searchWeb, type WebSearchResult } from "../ai/tools/webSearch.js";
import type { DiscoveredEpisode } from "./types.js";

const MAX_DISCOVERED = 12;

/**
 * Discovery angles, all constrained to the past week. Multi-modal on purpose:
 * each query surfaces episodes the others miss (community discussion,
 * curation lists, newsletters, topic searches).
 */
const DISCOVERY_QUERIES: {
  query: string;
  topic?: "general" | "news";
  timeRange: "day" | "week";
}[] = [
  { query: "best podcast episodes this week", timeRange: "week" },
  { query: "reddit standout podcast episode this week discussion", timeRange: "week" },
  { query: "podcast newsletter episode picks this week", timeRange: "week" },
  {
    query: "new podcast episode interview philosophy history science media criticism",
    timeRange: "week",
  },
  {
    query: "notable new podcast episode economics policy skepticism debate",
    timeRange: "week",
  },
];

const extractionSchema = z.object({
  episodes: z.array(
    z.object({
      show_title: z.string(),
      episode_title: z.string(),
      context: z
        .string()
        .describe("One line: where/why this surfaced (thread, list, newsletter)"),
      source_url: z.string().nullable(),
    }),
  ),
});

/**
 * Search the web for podcast episodes being talked about right now, then
 * extract a raw candidate list with the cheap model. Everything here is
 * unverified — release dates and identities are established later from the
 * shows' actual RSS feeds, never from search snippets.
 */
export async function discoverEpisodes(
  tasteDigest: string,
  recentRecommendationsDigest: string,
  logger: Logger,
  logFile?: LogFile,
): Promise<DiscoveredEpisode[]> {
  const searches = await Promise.all(
    DISCOVERY_QUERIES.map(async ({ query, topic, timeRange }) => {
      try {
        const { results } = await searchWeb({
          query,
          topic,
          timeRange,
          maxResults: 8,
          maxContentChars: 700,
        });
        return { query, results };
      } catch (error) {
        logger.warn(`Discovery search failed: ${query}`, (error as Error).message);
        return { query, results: [] as WebSearchResult[] };
      }
    }),
  );

  const resultCount = searches.reduce((sum, s) => sum + s.results.length, 0);
  if (resultCount === 0) {
    logger.warn("Discovery produced no search results");
    return [];
  }

  const { model, modelId } = getRecsShortlistModel();
  const prompt = buildExtractionPrompt(
    searches,
    tasteDigest,
    recentRecommendationsDigest,
  );
  logFile?.log(
    logger,
    LogLevel.INFO,
    `Discovery Extraction Prompt (${modelId})`,
    codeBlock(prompt),
    {
      consoleSummary: `Extracting candidates from ${resultCount} results (${modelId})`,
    },
  );

  const result = await generateText({
    model,
    output: Output.object({ schema: extractionSchema }),
    prompt,
  });
  logger.info(
    `Discovery token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  const seen = new Set<string>();
  const episodes: DiscoveredEpisode[] = [];
  for (const item of result.output?.episodes ?? []) {
    const key = `${item.show_title.toLowerCase()}::${item.episode_title.toLowerCase()}`;
    if (seen.has(key)) continue;
    seen.add(key);
    episodes.push({
      showTitle: item.show_title,
      episodeTitle: item.episode_title,
      context: item.context,
      sourceUrl: item.source_url ?? undefined,
    });
    if (episodes.length >= MAX_DISCOVERED) break;
  }

  logFile?.section(
    "Discovered Episodes",
    episodes
      .map((e) => `- ${e.showTitle} — ${e.episodeTitle} (${e.context})`)
      .join("\n") || "none",
  );
  return episodes;
}

function buildExtractionPrompt(
  searches: { query: string; results: WebSearchResult[] }[],
  tasteDigest: string,
  recentRecommendationsDigest: string,
): string {
  const blocks = searches
    .filter((s) => s.results.length > 0)
    .map((s) => {
      const lines = s.results
        .map((r) => `- ${r.title} (${r.url})\n  ${r.content.replace(/\s+/g, " ")}`)
        .join("\n");
      return `SEARCH: ${s.query}\n${lines}`;
    });

  return `You are extracting podcast episode candidates from web search results for one specific listener.

THE LISTENER:
${tasteDigest}

${recentRecommendationsDigest}

From the search results below, extract up to ${MAX_DISCOVERED} SPECIFIC podcast episodes (a show name AND an episode title/topic) that look like strong matches for this listener. Rules:
- Only episodes from shows the listener does NOT already subscribe to. The subscribed-show list above is an exclusion list AND taste evidence.
- Skip grifty, outrage-driven, influencer-style, true crime, lifestyle, and comedy/entertainment shows.
- Skip anything in the recently-recommended list.
- Prefer episodes with genuine discussion or curation behind them over algorithmic listicles.
- Episode titles may be approximate (they will be matched against the show's RSS feed later), but the show name must be as exact as possible.
- Do not invent episodes: every extraction must be traceable to a search result. Set source_url to the most relevant result URL, or null if none applies.

SEARCH RESULTS:
${blocks.join("\n\n")}

Return JSON only.`;
}
