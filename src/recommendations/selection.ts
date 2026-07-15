import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output } from "ai";
import { z } from "zod";
import { getRecsSelectionModel } from "../ai/registry.js";
import { searchWeb } from "../ai/tools/webSearch.js";
import type { ScoredCandidate } from "./shortlist.js";
import { formatCandidateDetails } from "./shortlist.js";

const notificationSchema = z.object({
  title: z
    .string()
    .describe("Short notification title prefixed with an emoji, e.g. '🎬 Dune (2021)'"),
  message: z
    .string()
    .describe("2-3 sentence notification body: why this pick, for this user"),
});

const pickSchema = z.object({
  candidate_id: z.string(),
  why_for_user: z.string(),
  caveats: z.array(z.string()),
  confidence: z.number().min(0).max(1),
  notification: notificationSchema,
});

const decisionSchema = z.object({
  decision: z.enum(["select", "no_add"]),
  selected: pickSchema.nullable(),
  backup: pickSchema
    .nullable()
    .describe(
      "Second choice with its OWN reasoning and notification copy; used only if the first is already on the watchlist",
    ),
  no_add_reason: z.string().nullable(),
});

export type SelectionPick = z.infer<typeof pickSchema>;
export type SelectionDecision = z.infer<typeof decisionSchema>;

/**
 * Research each finalist once with bounded Tavily snippets, then choose one
 * title or no_add in a single model call. Keeping research outside an agentic
 * loop avoids repeatedly billing the growing tool transcript on every step.
 */
export async function selectRecommendation(
  finalists: ScoredCandidate[],
  historyDigest: string,
  logger: Logger,
  logFile?: LogFile,
): Promise<SelectionDecision | undefined> {
  const { model, modelId } = getRecsSelectionModel();
  const research = await researchFinalists(finalists, logger, logFile);
  const prompt = buildPrompt(finalists, historyDigest, research);

  logFile?.log(
    logger,
    LogLevel.INFO,
    `Selection Prompt (${modelId})`,
    codeBlock(prompt),
    {
      consoleSummary: `Selecting from ${finalists.length} finalists (${modelId})`,
    },
  );

  const result = await generateText({
    model,
    output: Output.object({ schema: decisionSchema }),
    prompt,
  });
  logger.info(
    `Selection token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  const decision = result.output;
  if (decision) {
    logFile?.section(
      "Selection Decision",
      codeBlock(JSON.stringify(decision, null, 2), "json"),
    );
  }
  return decision ?? undefined;
}

async function researchFinalists(
  finalists: ScoredCandidate[],
  logger: Logger,
  logFile?: LogFile,
): Promise<Map<string, string>> {
  const entries = await Promise.all(
    finalists.map(async ({ candidate }) => {
      const query = `${candidate.title} ${candidate.year ?? ""} critical reception audience reviews ending quality`;
      logger.info(`Selection research: ${query}`);
      const response = await searchWeb({
        query,
        maxResults: 3,
        maxContentChars: 900,
      }).catch((error) => {
        logger.warn(`Research failed for ${candidate.title}`, (error as Error).message);
        return { results: [] };
      });
      const summary = response.results
        .map(
          (result) =>
            `- ${result.title} (${result.url})\n  ${result.content.replace(/\s+/g, " ")}`,
        )
        .join("\n");
      logFile?.section(`Research: ${candidate.title}`, summary || "No results");
      return [
        candidate.canonicalId,
        summary || "No research results available.",
      ] as const;
    }),
  );
  return new Map(entries);
}

function buildPrompt(
  finalists: ScoredCandidate[],
  historyDigest: string,
  research: Map<string, string>,
): string {
  const finalistBlocks = finalists.map((s) => {
    const c = s.candidate;
    const year = c.year ? ` (${c.year})` : "";
    const genres = c.genres.join("/") || "unknown genres";
    const library = c.inLibrary ? "\n  Already available in the local library." : "";
    const risks =
      s.risks.length > 0 ? `\n  Pre-screening risk flags: ${s.risks.join("; ")}` : "";
    const details = formatCandidateDetails(c);
    return `[${c.canonicalId}] ${c.title}${year} [${c.mediaType}] ${genres} | TMDB rating ${c.voteAverage.toFixed(1)} (${c.voteCount} votes)${details}${library}${risks}\n  ${c.overview.replace(/\s+/g, " ").slice(0, 400)}\n  Research:\n${research.get(c.canonicalId)}`;
  });

  return `You are choosing at most ONE title to add to one person's media watchlist today. Your job is precision, not activity: a skipped day costs nothing; a bad pick erodes trust in every future recommendation.

THE USER'S TASTE EVIDENCE (watch history is ground truth; explicit good/not-for-me feedback is direct preference evidence):
${historyDigest}

FINALISTS (pre-screened for taste fit):
${finalistBlocks.join("\n\n")}

PROCESS:
1. Evaluate the supplied research for critical reception, audience sentiment, whether the title holds up, and material caveats.
2. Compare against the user's actual watch history — justify against what they demonstrably watch and finish, not generic acclaim.
3. Decide: select exactly one, or no_add if the evidence is weak for all finalists. Prefer the smaller, higher-confidence commitment when two options are similarly good. no_add is a respectable outcome, not a failure.

Return the structured decision. Candidate ids must come from the list above. The backup is your second choice, used only if the first turns out to already be on the watchlist, so give it its own honest why_for_user and notification copy (never reuse the primary's). why_for_user should reference their actual watching patterns. Keep notification messages concise and concrete. For no_add, set selected and backup to null and explain in no_add_reason.`;
}
