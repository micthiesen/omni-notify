import type { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock } from "@micthiesen/mitools/markdown";
import { generateText, Output } from "ai";
import { z } from "zod";
import { getRecsSelectionModel } from "../ai/registry.js";
import { toDateStamp } from "../utils/dates.js";
import type { PodcastSelectionPick } from "./selection.js";
import type { EpisodeCandidate } from "./types.js";

const decisionSchema = z.object({
  decisions: z.array(
    z.object({
      candidate_id: z.string(),
      include: z.boolean(),
      reason: z.string(),
      why_for_user: z.string().nullable(),
      caveats: z.array(z.string()),
      confidence: z.number().min(0).max(1),
      notification: z.object({ title: z.string(), message: z.string() }).nullable(),
    }),
  ),
});

export type GuestDecision = z.infer<typeof decisionSchema>["decisions"][number];

export interface GuestPick {
  candidate: EpisodeCandidate;
  pick: PodcastSelectionPick;
}

/**
 * Reduce the model's decisions to committable picks. Pure and defensive against
 * ragged model output: keep only includes, strongest-confidence first, dedup by
 * candidate_id (a repeat would otherwise double-commit + double-enqueue the same
 * episode), drop unknown ids and includes missing their copy, then cap at `max`.
 */
export function applyGuestDecisions(
  decisions: GuestDecision[],
  candidatesById: Map<string, EpisodeCandidate>,
  max: number,
  logger?: Logger,
): GuestPick[] {
  const included = decisions
    .filter((decision) => decision.include)
    .sort((a, b) => b.confidence - a.confidence);

  const picks: GuestPick[] = [];
  const seen = new Set<string>();
  for (const decision of included) {
    if (picks.length >= max) break;
    if (seen.has(decision.candidate_id)) continue;
    seen.add(decision.candidate_id);

    const candidate = candidatesById.get(decision.candidate_id);
    if (!candidate) {
      logger?.warn(
        `Guest gate returned unknown candidate_id: ${decision.candidate_id}`,
      );
      continue;
    }
    if (!decision.why_for_user || !decision.notification) {
      logger?.warn(
        `Guest gate included ${candidate.showTitle} — ${candidate.episodeTitle} without copy; skipping`,
      );
      continue;
    }
    picks.push({
      candidate,
      pick: {
        candidate_id: decision.candidate_id,
        why_for_user: decision.why_for_user,
        caveats: decision.caveats,
        confidence: decision.confidence,
        notification: decision.notification,
      },
    });
  }
  return picks;
}

/**
 * Tier-1 gate for guest appearances of followed voices. The listener already
 * follows these people, so this DEFAULTS TO INCLUDE and only drops episodes
 * that are off-taste or trivial. Unlike the Tier-2 selector it is inclusive
 * (all survivors up to `max`), which is what lets a press-tour week surface
 * several appearances at once.
 */
export async function selectGuestAppearances(
  candidates: EpisodeCandidate[],
  tasteDigest: string,
  logger: Logger,
  logFile: LogFile | undefined,
  max: number,
): Promise<GuestPick[]> {
  if (candidates.length === 0) return [];
  const { model, modelId } = getRecsSelectionModel("select-guest-appearances");
  const prompt = buildPrompt(candidates, tasteDigest, max);

  logFile?.log(
    logger,
    LogLevel.INFO,
    `Guest Gate Prompt (${modelId})`,
    codeBlock(prompt),
    { consoleSummary: `Gating ${candidates.length} guest candidate(s) (${modelId})` },
  );

  const result = await generateText({
    model,
    output: Output.object({ schema: decisionSchema }),
    prompt,
  });
  logger.info(
    `Guest gate token usage: ${result.usage.inputTokens} prompt, ${result.usage.outputTokens} completion`,
  );

  const byId = new Map(candidates.map((c) => [c.episodeId, c]));
  const picks = applyGuestDecisions(result.output?.decisions ?? [], byId, max, logger);

  logFile?.section(
    "Guest Gate",
    picks
      .map((p) => `INCLUDE ${p.candidate.showTitle} — ${p.candidate.episodeTitle}`)
      .join("\n") || "none included",
  );
  return picks;
}

function buildPrompt(
  candidates: EpisodeCandidate[],
  tasteDigest: string,
  max: number,
): string {
  const blocks = candidates.map((c) => {
    const released = toDateStamp(c.publishedAt);
    const duration = c.durationMinutes ? ` | ${c.durationMinutes} min` : "";
    return `[${c.episodeId}] ${c.showTitle} — ${c.episodeTitle} | featuring: ${(c.matchedVoices ?? []).join(", ")} | released ${released}${duration}\n  ${c.description.replace(/\s+/g, " ").slice(0, 300)}`;
  });

  return `You are gating podcast episodes where a voice the listener FOLLOWS appears as a guest. Following the person is a strong positive signal, so DEFAULT TO INCLUDE — only exclude an episode if it is clearly off-taste (grift/guru/outrage/rage-farming/promotional or sponsored), clearly trivial (a brief mention, a rerun, not a real substantive appearance), or clearly a NAMESAKE (a different person who happens to share the followed voice's name — e.g. a pastor named Sean Carroll is not the physicist). Exclude namesakes.

THE LISTENER:
${tasteDigest}

Decide include (default true) or exclude for each candidate. Include at most ${max}; if more qualify, keep the strongest. For each INCLUDED episode, write why_for_user (reference the followed guest and the listener's taste) and a notification: a title prefixed with a topic emoji and naming the show + guest, plus a concrete 2-3 sentence message. For EXCLUDED episodes set why_for_user and notification to null and give a short reason.

CANDIDATES:
${blocks.join("\n")}

Return JSON only.`;
}
