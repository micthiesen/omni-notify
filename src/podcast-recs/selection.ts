import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output } from "ai";
import { z } from "zod";
import { getRecsSelectionModel } from "../ai/registry.js";
import { searchWeb } from "../ai/tools/webSearch.js";
import type { ScoredEpisode } from "./shortlist.js";

const notificationSchema = z.object({
  title: z
    .string()
    .describe(
      "Short notification title prefixed with a topic emoji, e.g. '🏛️ The Rest Is Politics — Inside the Election'",
    ),
  message: z
    .string()
    .describe("2-3 sentence notification body: why this episode, for this listener"),
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
  no_add_reason: z.string().nullable(),
});

export type PodcastSelectionPick = z.infer<typeof pickSchema>;
export type PodcastSelectionDecision = z.infer<typeof decisionSchema>;

/**
 * Research each finalist once with bounded Tavily snippets, then choose one
 * episode (or no_add) per call against a shrinking finalist set — the same
 * research-outside-the-loop shape as the media recommendations selector.
 */
export async function selectEpisode(
  finalists: ScoredEpisode[],
  tasteDigest: string,
  research: Map<string, string>,
  logger: Logger,
  logFile?: LogFile,
): Promise<PodcastSelectionDecision | undefined> {
  const { model, modelId } = getRecsSelectionModel();
  const prompt = buildPrompt(finalists, tasteDigest, research);

  logFile?.log(
    logger,
    LogLevel.INFO,
    `Podcast Selection Prompt (${modelId})`,
    codeBlock(prompt),
    { consoleSummary: `Selecting from ${finalists.length} finalists (${modelId})` },
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
      "Podcast Selection Decision",
      codeBlock(JSON.stringify(decision, null, 2), "json"),
    );
  }
  return decision ?? undefined;
}

export async function researchFinalists(
  finalists: ScoredEpisode[],
  logger: Logger,
  logFile?: LogFile,
): Promise<Map<string, string>> {
  const entries = await Promise.all(
    finalists.map(async ({ candidate }) => {
      const query = `"${candidate.showTitle}" podcast ${candidate.episodeTitle} review discussion`;
      logger.info(`Selection research: ${query}`);
      const response = await searchWeb({
        query,
        maxResults: 3,
        maxContentChars: 800,
      }).catch((error) => {
        logger.warn(
          `Research failed for ${candidate.episodeTitle}`,
          (error as Error).message,
        );
        return { results: [] };
      });
      const summary = response.results
        .map(
          (result) =>
            `- ${result.title} (${result.url})\n  ${result.content.replace(/\s+/g, " ")}`,
        )
        .join("\n");
      logFile?.section(
        `Research: ${candidate.showTitle} — ${candidate.episodeTitle}`,
        summary || "No results",
      );
      return [
        candidate.episodeId,
        summary || "No research results available.",
      ] as const;
    }),
  );
  return new Map(entries);
}

function buildPrompt(
  finalists: ScoredEpisode[],
  tasteDigest: string,
  research: Map<string, string>,
): string {
  const finalistBlocks = finalists.map((s) => {
    const c = s.candidate;
    const released = new Date(c.publishedAt).toISOString().slice(0, 10);
    const duration = c.durationMinutes ? ` | ${c.durationMinutes} min` : "";
    const genres = c.showGenres.join("/") || "unknown genres";
    const risks =
      s.risks.length > 0 ? `\n  Pre-screening risk flags: ${s.risks.join("; ")}` : "";
    return `[${c.episodeId}] ${c.showTitle} — ${c.episodeTitle} | ${genres} | released ${released}${duration}\n  Surfaced via: ${c.discoveredVia}${risks}\n  ${c.description.replace(/\s+/g, " ").slice(0, 400)}\n  Research:\n${research.get(c.episodeId)}`;
  });

  return `You are choosing at most ONE podcast episode to recommend to one person today. The listener LIKES having lots of options and would rather hear about a good episode than get nothing, so LEAN TOWARD RECOMMENDING: pick the best available finalist unless they are all genuinely weak or off-taste. A good, on-taste episode is worth surfacing even if it isn't a perfect standout — only skip when nothing here is a reasonable fit.

This is the TOPIC/standout tier. The listener's "guest appearances of people I follow" priority is handled by a SEPARATE stage — do NOT penalize a candidate here for lacking a followed voice. Judge purely on whether it's a solid, on-taste episode worth their time.

THE LISTENER (subscribed shows are ground truth; explicit feedback is direct preference evidence):
${tasteDigest}

FINALISTS (identities and release dates already verified against each show's RSS feed):
${finalistBlocks.join("\n\n")}

PROCESS:
1. Evaluate the research for genuine buzz vs promotional noise, host credibility, and whether the episode stands alone for a first-time listener of that show.
2. Compare against the listener's subscribed shows and feedback — justify against what they demonstrably listen to, not generic acclaim. A smart, on-taste episode from a show adjacent to what they love is a good pick even without manufactured drama.
3. Decide: select the strongest finalist. Use no_add ONLY when every finalist is genuinely weak or off-taste — not merely because none is a perfect standout, and never because a description doesn't prove sharp conflict.

Return the structured decision. Candidate ids must come from the list above. why_for_user should reference their actual listening patterns. Keep the notification concise and concrete, with a topic-appropriate emoji prefix on the title. For no_add, set selected to null and explain in no_add_reason.`;
}
