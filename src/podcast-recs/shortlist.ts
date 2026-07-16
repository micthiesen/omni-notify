import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output } from "ai";
import { z } from "zod";
import { getRecsShortlistModel } from "../ai/registry.js";
import type { EpisodeCandidate } from "./types.js";

export const FINALIST_COUNT = 4;

const scoreSchema = z.object({
  scores: z.array(
    z.object({
      candidate_id: z.string(),
      taste_match: z.number().min(0).max(100),
      novelty: z.number().min(0).max(100),
      confidence: z.number().min(0).max(1),
      risks: z.array(z.string()),
    }),
  ),
});

export interface ScoredEpisode {
  candidate: EpisodeCandidate;
  tasteMatch: number;
  novelty: number;
  confidence: number;
  risks: string[];
  composite: number;
}

/**
 * Composite ranking score, computed in code (never trust model prose for
 * ordering). Freshness is a hard filter, not a score; duration matters little
 * for podcasts, so taste dominates with novelty as the tiebreaker.
 */
export function computeComposite(score: {
  tasteMatch: number;
  novelty: number;
  confidence: number;
}): number {
  const base = 0.7 * score.tasteMatch + 0.3 * score.novelty;
  return base * (0.5 + 0.5 * score.confidence);
}

/** Score all eligible episodes with the cheap model and keep the top N. */
export async function shortlistEpisodes(
  candidates: EpisodeCandidate[],
  tasteDigest: string,
  logger: Logger,
  logFile?: LogFile,
  finalistCount = FINALIST_COUNT,
): Promise<ScoredEpisode[]> {
  const { model, modelId } = getRecsShortlistModel();

  // Deterministic ordering to reduce position bias.
  const ordered = [...candidates].sort((a, b) =>
    a.episodeId.localeCompare(b.episodeId),
  );
  const prompt = buildPrompt(ordered, tasteDigest);

  logFile?.log(
    logger,
    LogLevel.INFO,
    `Podcast Shortlist Prompt (${modelId})`,
    codeBlock(prompt),
    { consoleSummary: `Scoring ${ordered.length} episodes (${modelId})` },
  );

  const result = await generateText({
    model,
    output: Output.object({ schema: scoreSchema }),
    prompt,
  });
  logger.info(
    `Shortlist token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  const byId = new Map(candidates.map((c) => [c.episodeId, c]));
  const scored: ScoredEpisode[] = [];
  for (const score of result.output?.scores ?? []) {
    const candidate = byId.get(score.candidate_id);
    if (!candidate) {
      logger.warn(`Shortlist returned unknown candidate_id: ${score.candidate_id}`);
      continue;
    }
    scored.push({
      candidate,
      tasteMatch: score.taste_match,
      novelty: score.novelty,
      confidence: score.confidence,
      risks: score.risks,
      composite: computeComposite({
        tasteMatch: score.taste_match,
        novelty: score.novelty,
        confidence: score.confidence,
      }),
    });
  }

  scored.sort((a, b) => b.composite - a.composite);
  const finalists = scored.slice(0, finalistCount);

  logFile?.section(
    "Podcast Shortlist Result",
    codeBlock(
      finalists
        .map(
          (s) =>
            `${s.composite.toFixed(1)} ${s.candidate.showTitle} — ${s.candidate.episodeTitle} (taste=${s.tasteMatch} novelty=${s.novelty} conf=${s.confidence})`,
        )
        .join("\n"),
    ),
  );

  return finalists;
}

function buildPrompt(candidates: EpisodeCandidate[], tasteDigest: string): string {
  const candidateLines = candidates.map((c) => {
    const duration = c.durationMinutes ? ` | ${c.durationMinutes} min` : "";
    const genres = c.showGenres.length > 0 ? c.showGenres.join("/") : "unknown genres";
    const released = new Date(c.publishedAt).toISOString().slice(0, 10);
    const description = c.description.replace(/\s+/g, " ").slice(0, 220);
    return `[${c.episodeId}] ${c.showTitle} — ${c.episodeTitle} | ${genres} | released ${released}${duration} | surfaced via: ${c.discoveredVia}\n  ${description}`;
  });

  return `You are a conservative scorer of podcast EPISODES for one specific listener. You are NOT choosing a winner — score each candidate from the supplied facts only. Do not invent metadata.

THE LISTENER (subscribed shows are ground truth; explicit feedback is direct preference evidence):
${tasteDigest}

SCORING DIMENSIONS (0-100 each):
- taste_match: fit with the tastes evident in the subscribed shows and profile (long-form intellectual conversation over vibes; not generic popularity).
- novelty: rewards shows/hosts/perspectives adjacent-but-new relative to what they already follow; penalize both near-clones of subscribed shows and total non-sequiturs.

Also return confidence (0-1) in your own scoring, and risk flags (e.g. "possibly promotional", "host known for outrage content", "episode topic may be stale news").

Score every candidate. Return JSON only.

CANDIDATES:
${candidateLines.join("\n")}`;
}
