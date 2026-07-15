import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output } from "ai";
import { z } from "zod";
import { getRecsShortlistModel } from "../ai/registry.js";
import type { Candidate } from "./types.js";

export const FINALIST_COUNT = 5;

const scoreSchema = z.object({
  scores: z.array(
    z.object({
      candidate_id: z.string(),
      taste_match: z.number().min(0).max(100),
      novelty: z.number().min(0).max(100),
      effort_fit: z.number().min(0).max(100),
      confidence: z.number().min(0).max(1),
      risks: z.array(z.string()),
    }),
  ),
});

export interface ScoredCandidate {
  candidate: Candidate;
  tasteMatch: number;
  novelty: number;
  effortFit: number;
  confidence: number;
  risks: string[];
  composite: number;
}

/**
 * Composite ranking score, computed in code (never trust model prose for
 * ordering). Confidence shrinks the score toward the middle rather than
 * multiplying it away entirely.
 */
export function computeComposite(score: {
  tasteMatch: number;
  novelty: number;
  effortFit: number;
  confidence: number;
}): number {
  const base = 0.55 * score.tasteMatch + 0.25 * score.novelty + 0.2 * score.effortFit;
  return base * (0.5 + 0.5 * score.confidence);
}

/** Score all eligible candidates with the cheap model and keep the top N. */
export async function shortlistCandidates(
  candidates: Candidate[],
  historyDigest: string,
  logger: Logger,
  logFile?: LogFile,
): Promise<ScoredCandidate[]> {
  const { model, modelId } = getRecsShortlistModel();

  // Deterministic, popularity-agnostic ordering to reduce position bias.
  const ordered = [...candidates].sort((a, b) => a.tmdbId - b.tmdbId);
  const prompt = buildPrompt(ordered, historyDigest);

  logFile?.log(
    logger,
    LogLevel.INFO,
    `Shortlist Prompt (${modelId})`,
    codeBlock(prompt),
    {
      consoleSummary: `Scoring ${ordered.length} candidates (${modelId})`,
    },
  );

  const result = await generateText({
    model,
    output: Output.object({ schema: scoreSchema }),
    prompt,
  });
  logger.info(
    `Shortlist token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  const byId = new Map<string, Candidate>(candidates.map((c) => [c.canonicalId, c]));
  const scored: ScoredCandidate[] = [];
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
      effortFit: score.effort_fit,
      confidence: score.confidence,
      risks: score.risks,
      composite: computeComposite({
        tasteMatch: score.taste_match,
        novelty: score.novelty,
        effortFit: score.effort_fit,
        confidence: score.confidence,
      }),
    });
  }

  scored.sort((a, b) => b.composite - a.composite);
  const finalists = scored.slice(0, FINALIST_COUNT);

  logFile?.section(
    "Shortlist Result",
    codeBlock(
      finalists
        .map(
          (s) =>
            `${s.composite.toFixed(1)} ${s.candidate.title} (taste=${s.tasteMatch} novelty=${s.novelty} effort=${s.effortFit} conf=${s.confidence})`,
        )
        .join("\n"),
    ),
  );

  return finalists;
}

function buildPrompt(candidates: Candidate[], historyDigest: string): string {
  const candidateLines = candidates.map((c) => {
    const year = c.year ? ` (${c.year})` : "";
    const genres = c.genres.length > 0 ? c.genres.join("/") : "unknown genres";
    const library = c.inLibrary ? " | IN LOCAL LIBRARY" : "";
    const overview = c.overview.replace(/\s+/g, " ").slice(0, 180);
    const details = formatCandidateDetails(c, false);
    return `[${c.canonicalId}] ${c.title}${year} [${c.mediaType}] ${genres} | rating ${c.voteAverage.toFixed(1)} (${c.voteCount} votes) | source=${c.source}${library}${details}\n  ${overview}`;
  });

  return `You are a conservative recommendation scorer for one person's media watchlist. You are NOT choosing a winner — you are scoring each candidate from the supplied facts only. Do not invent metadata or use knowledge that contradicts the provided facts.

THE USER'S TASTE EVIDENCE (watch history is ground truth; explicit good/not-for-me feedback is direct preference evidence):
${historyDigest}

SCORING DIMENSIONS (0-100 each):
- taste_match: how well this fits the tastes evident in the watch history (not generic acclaim).
- novelty: rewards fresh-but-plausible territory; penalize both carbon copies of recent watches and wild leaps with no anchor in the history.
- effort_fit: how likely they are to actually start it soon. Movies and limited series score higher than huge multi-season commitments unless the history shows they finish long shows.

Also return confidence (0-1) in your own scoring for that candidate, and any risk flags (e.g. "long commitment", "divisive reception", "very similar to X they just watched").

Score every candidate. Return JSON only.

CANDIDATES:
${candidateLines.join("\n")}`;
}

export function formatCandidateDetails(
  candidate: Candidate,
  includeCreativeContext = true,
): string {
  const details: string[] = [];
  if (candidate.runtimeMinutes) {
    details.push(
      candidate.mediaType === "tv"
        ? `${candidate.runtimeMinutes} min/episode`
        : `${candidate.runtimeMinutes} min`,
    );
  }
  if (candidate.seasonCount) details.push(`${candidate.seasonCount} seasons`);
  if (candidate.episodeCount) details.push(`${candidate.episodeCount} episodes`);
  if (candidate.seriesStatus) details.push(candidate.seriesStatus);
  if (candidate.certification) details.push(candidate.certification);
  if (candidate.originalLanguage && candidate.originalLanguage !== "en") {
    details.push(`language=${candidate.originalLanguage}`);
  }
  if (candidate.originCountries?.length) {
    details.push(`origin=${candidate.originCountries.join(",")}`);
  }
  if (includeCreativeContext) {
    if (candidate.creators?.length) {
      details.push(`creator=${candidate.creators.join(", ")}`);
    }
    if (candidate.cast?.length) {
      details.push(`cast=${candidate.cast.slice(0, 4).join(", ")}`);
    }
    if (candidate.keywords?.length) {
      details.push(`themes=${candidate.keywords.slice(0, 6).join(", ")}`);
    }
  }
  return details.length > 0 ? ` | ${details.join(" | ")}` : "";
}
