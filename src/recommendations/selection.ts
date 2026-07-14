import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, isStepCount, Output } from "ai";
import { z } from "zod";
import { getRecsSelectionModel } from "../ai/registry.js";
import { fetchUrl } from "../ai/tools/fetchUrl.js";
import { webSearch } from "../ai/tools/webSearch.js";
import type { ScoredCandidate } from "./shortlist.js";

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
 * Research the finalists (web search + page fetches) and choose exactly one
 * title, or no_add. Combined research+selection in a single agentic loop —
 * the strong model only ever sees the <=5 researched finalists.
 */
export async function selectRecommendation(
  finalists: ScoredCandidate[],
  historyDigest: string,
  logger: Logger,
  logFile?: LogFile,
): Promise<SelectionDecision | undefined> {
  const { model, modelId } = getRecsSelectionModel();
  const prompt = buildPrompt(finalists, historyDigest);

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
    tools: { web_search: webSearch, fetch_url: fetchUrl },
    stopWhen: isStepCount(24),
    output: Output.object({ schema: decisionSchema }),
    onStepFinish: ({ toolCalls }) => {
      for (const call of toolCalls) {
        const input = call.input as { query?: string; url?: string };
        logger.info(
          `Selection tool: ${call.toolName}(${input.query ?? input.url ?? ""})`,
        );
        logFile?.section(
          `Tool Call: ${call.toolName}`,
          codeBlock(JSON.stringify(call.input, null, 2), "json"),
        );
      }
    },
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

function buildPrompt(finalists: ScoredCandidate[], historyDigest: string): string {
  const finalistBlocks = finalists.map((s) => {
    const c = s.candidate;
    const year = c.year ? ` (${c.year})` : "";
    const genres = c.genres.join("/") || "unknown genres";
    const library = c.inLibrary ? "\n  Already available in the local library." : "";
    const risks =
      s.risks.length > 0 ? `\n  Pre-screening risk flags: ${s.risks.join("; ")}` : "";
    return `[${c.canonicalId}] ${c.title}${year} [${c.mediaType}] ${genres} | TMDB rating ${c.voteAverage.toFixed(1)} (${c.voteCount} votes)${library}${risks}\n  ${c.overview.replace(/\s+/g, " ").slice(0, 400)}`;
  });

  return `You are choosing at most ONE title to add to one person's media watchlist today. Your job is precision, not activity: a skipped day costs nothing; a bad pick erodes trust in every future recommendation.

THE USER'S WATCH HISTORY (ground truth for their taste):
${historyDigest}

FINALISTS (pre-screened for taste fit):
${finalistBlocks.join("\n\n")}

PROCESS:
1. Research each finalist with web_search (and fetch_url for promising pages): critical reception, audience sentiment, whether it holds up / how it ends, anything recent (renewal, cancellation, re-release) that changes its appeal.
2. Compare against the user's actual watch history — justify against what they demonstrably watch and finish, not generic acclaim.
3. Decide: select exactly one, or no_add if the evidence is weak for all finalists. Prefer the smaller, higher-confidence commitment when two options are similarly good. no_add is a respectable outcome, not a failure.

Return the structured decision. Candidate ids must come from the list above. The backup is your second choice, used only if the first turns out to already be on the watchlist, so give it its own honest why_for_user and notification copy (never reuse the primary's). why_for_user should reference their actual watching patterns. Keep notification messages concise and concrete. For no_add, set selected and backup to null and explain in no_add_reason.`;
}
