import { generateText, Output } from "ai";
import { z } from "zod";
import { toDateStamp } from "../../utils/dates.js";
import { PodcastRecommendationStatus } from "../persistence.js";
import {
  deriveListenEvidence,
  deriveRecommendationEvidence,
  fingerprintEvidence,
} from "./evidence.js";
import {
  getAllPodcastTasteEvidence,
  getLatestPodcastTasteProfile,
  insertPodcastTasteEvidence,
  insertPodcastTasteProfile,
} from "./persistence.js";
import { computePodcastBehavioralStats } from "./stats.js";
import type {
  PodcastTasteClaim,
  PodcastTasteEvidenceData,
  PodcastTasteProfileContent,
  PodcastTasteProfileData,
  PodcastTasteReflectionInput,
  PodcastTasteReflectionResult,
} from "./types.js";

export const PODCAST_TASTE_PROMPT_VERSION = "podcast-taste-reflection-v1";
const DEFAULT_MAX_EVIDENCE = 160;

const claimSchema = z.object({
  claim: z.string().min(1).max(240),
  confidence: z.number().min(0).max(1),
  evidence_ids: z.array(z.string()).max(12),
});

const profileSchema = z.object({
  stable_preferences: z.array(claimSchema).max(10),
  conditional_preferences: z.array(claimSchema).max(10),
  aversions: z.array(claimSchema).max(10),
  current_saturation: z.array(claimSchema).max(8),
  exploration_targets: z.array(claimSchema).max(8),
  uncertainties: z.array(claimSchema).max(8),
});

type RawProfile = z.infer<typeof profileSchema>;

/**
 * Sibling of the media taste reflection (recommendations/taste/reflection.ts):
 * append current observations, checkpoint on their fingerprint, then run a
 * draft and a skeptical revision. Model output is constrained and claims that
 * cite missing or inadequate evidence are removed before persistence.
 */
export async function runPodcastTasteReflection(
  input: PodcastTasteReflectionInput,
): Promise<PodcastTasteReflectionResult> {
  const incoming = [
    ...deriveListenEvidence(input.listened),
    ...deriveRecommendationEvidence(input.recommendations),
  ];
  const insertedEvidence = insertPodcastTasteEvidence(incoming);
  const allEvidence = getAllPodcastTasteEvidence();
  if (allEvidence.length === 0) {
    return { status: "insufficient_evidence", insertedEvidence };
  }

  const evidenceFingerprint = fingerprintEvidence(allEvidence);
  const latest = getLatestPodcastTasteProfile();
  if (latest?.evidenceFingerprint === evidenceFingerprint) {
    return { status: "unchanged", profile: latest, insertedEvidence };
  }

  const boundedEvidence = selectPodcastReflectionEvidence(
    allEvidence,
    input.maxEvidence ?? DEFAULT_MAX_EVIDENCE,
  );
  const stats = computePodcastBehavioralStats(allEvidence);
  const evidenceJson = JSON.stringify(boundedEvidence.map(compactEvidence), null, 2);
  const draftResult = await generateText({
    model: input.model,
    output: Output.object({ schema: profileSchema }),
    prompt: buildDraftPrompt(evidenceJson, stats),
  });
  if (!draftResult.output)
    throw new Error("Podcast taste reflection returned no draft");

  const finalResult = await generateText({
    model: input.model,
    output: Output.object({ schema: profileSchema }),
    prompt: buildCriticPrompt(
      evidenceJson,
      stats,
      JSON.stringify(draftResult.output, null, 2),
    ),
  });
  if (!finalResult.output) {
    throw new Error("Podcast taste reflection returned no revision");
  }

  const validated = validatePodcastProfile(finalResult.output, allEvidence);
  const generatedAt = input.now ?? Date.now();
  const version = (latest?.version ?? 0) + 1;
  const profile: PodcastTasteProfileData = {
    ...validated.profile,
    profileId: `v${version}:${evidenceFingerprint}`,
    version,
    generatedAt,
    evidenceFingerprint,
    evidenceCount: allEvidence.length,
    modelId: input.modelId,
    promptVersion: PODCAST_TASTE_PROMPT_VERSION,
    stats,
  };
  insertPodcastTasteProfile(profile);
  return {
    status: "created",
    profile,
    insertedEvidence,
    rejectedClaims: validated.rejectedClaims,
  };
}

/** Prefer direct feedback, then recommendation outcomes, then recent listens. */
export function selectPodcastReflectionEvidence(
  evidence: PodcastTasteEvidenceData[],
  limit: number,
): PodcastTasteEvidenceData[] {
  if (limit <= 0) return [];
  const weight = (item: PodcastTasteEvidenceData) => {
    if (item.kind === "explicit_feedback") return 4;
    if (
      item.recommendationStatus === PodcastRecommendationStatus.Listened ||
      item.recommendationStatus === PodcastRecommendationStatus.Abandoned ||
      item.recommendationStatus === PodcastRecommendationStatus.Ignored
    )
      return 3;
    if (item.kind === "listen") return 2;
    return 1;
  };
  return [...evidence]
    .sort(
      (a, b) =>
        weight(b) - weight(a) ||
        b.observedAt - a.observedAt ||
        a.evidenceId.localeCompare(b.evidenceId),
    )
    .slice(0, limit);
}

export function validatePodcastProfile(
  raw: RawProfile,
  evidence: PodcastTasteEvidenceData[],
): { profile: PodcastTasteProfileContent; rejectedClaims: number } {
  const byId = new Map(evidence.map((item) => [item.evidenceId, item]));
  let rejectedClaims = 0;
  const validateClaims = (
    claims: RawProfile["stable_preferences"],
    minimumShows: number,
    allowSingleExplicitAversion = false,
  ): PodcastTasteClaim[] =>
    claims.flatMap((claim) => {
      const evidenceIds = [...new Set(claim.evidence_ids)].filter((id) => {
        const item = byId.get(id);
        return item !== undefined && isTasteBearingEvidence(item);
      });
      const hasExplicitNegative = evidenceIds.some(
        (id) => byId.get(id)?.feedback === "not_for_me",
      );
      const independentShows = new Set(
        evidenceIds.map((id) => byId.get(id)?.showKey).filter(Boolean),
      ).size;
      const enough =
        independentShows >= minimumShows ||
        (allowSingleExplicitAversion && hasExplicitNegative);
      if (!enough) {
        rejectedClaims++;
        return [];
      }
      return [{ claim: claim.claim, confidence: claim.confidence, evidenceIds }];
    });

  const stablePreferences = validateClaims(raw.stable_preferences, 2);
  const conditionalPreferences = validateClaims(raw.conditional_preferences, 2);
  const aversions = validateClaims(raw.aversions, 2, true);
  const currentSaturation = validateClaims(raw.current_saturation, 2);
  const explorationTargets = validateClaims(raw.exploration_targets, 1);
  const uncertainties = validateClaims(raw.uncertainties, 1);
  const summaryParts = [
    stablePreferences.length > 0
      ? `Evidence-backed preferences: ${stablePreferences.map((item) => item.claim).join("; ")}.`
      : "Podcast taste evidence is still limited.",
    aversions.length > 0
      ? `Evidence-backed aversions: ${aversions.map((item) => item.claim).join("; ")}.`
      : undefined,
  ].filter((part): part is string => Boolean(part));

  return {
    profile: {
      summary: summaryParts.join(" "),
      stablePreferences,
      conditionalPreferences,
      aversions,
      currentSaturation,
      explorationTargets,
      uncertainties,
    },
    rejectedClaims,
  };
}

/**
 * A listen supports taste claims when it was finished (≥80%) or the client
 * reported a playback event without completion data; a bare few-minute sample
 * is too ambiguous. Starred episodes always count.
 */
function isTasteBearingEvidence(item: PodcastTasteEvidenceData): boolean {
  if (item.kind === "explicit_feedback") {
    // A note-only row (no good_pick/not_for_me) still reaches reflection so
    // its interpretive context isn't lost, but it's bound to exactly one
    // showKey like any other explicit_feedback row, so it can't alone
    // satisfy the ≥2-independent-show threshold in validateClaims.
    return (
      item.feedback === "good_pick" ||
      item.feedback === "not_for_me" ||
      Boolean(item.note)
    );
  }
  if (item.kind === "recommendation_outcome") {
    return (
      item.recommendationStatus === PodcastRecommendationStatus.Listened ||
      item.recommendationStatus === PodcastRecommendationStatus.Abandoned ||
      item.recommendationStatus === PodcastRecommendationStatus.Ignored
    );
  }
  if (item.starred === true) return true;
  return item.completion === undefined || item.completion >= 0.8;
}

export function formatPodcastTasteProfileDigest(
  profile: PodcastTasteProfileData | undefined = getLatestPodcastTasteProfile(),
): string {
  if (!profile) return "No reflective podcast taste profile is available yet.";
  const claimLines = (label: string, claims: PodcastTasteClaim[]) =>
    claims.length > 0
      ? `${label}: ${claims.map((item) => item.claim).join("; ")}`
      : undefined;
  return [
    `Reflective podcast taste profile v${profile.version}: ${profile.summary}`,
    claimLines("Stable preferences", profile.stablePreferences),
    claimLines("Conditional preferences", profile.conditionalPreferences),
    claimLines("Aversions", profile.aversions),
    claimLines("Current saturation", profile.currentSaturation),
    claimLines("Exploration targets", profile.explorationTargets),
    claimLines("Uncertainties", profile.uncertainties),
  ]
    .filter(Boolean)
    .join("\n");
}

function compactEvidence(item: PodcastTasteEvidenceData): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries({
      id: item.evidenceId,
      kind: item.kind,
      show: item.showTitle,
      episode: item.episodeTitle,
      observed_at: toDateStamp(item.observedAt),
      completion: item.completion,
      starred: item.starred,
      recommendation_status: item.recommendationStatus,
      feedback: item.feedback,
      note: item.note,
      discovered_via: item.discoveredVia,
      matched_voices: item.matchedVoices,
      duration_minutes: item.durationMinutes,
    }).filter(([, value]) => value !== undefined),
  );
}

function buildDraftPrompt(evidenceJson: string, stats: unknown): string {
  return `Build a conservative, useful taste profile for one person's podcast-episode recommendation system.

Context: the system recommends episodes of shows the listener does NOT already follow (subscriptions are handled elsewhere). This profile is about what the evidence shows they finish, star, bail on, and explicitly rate.

Rules:
- Infer preferences from demonstrated behavior, not popularity or stereotypes.
- Starred episodes and explicit feedback are strongest. A finished episode is positive-but-ambiguous. A low-completion listen is weak; an abandoned recommendation is a real negative. Ignored is weak evidence. Pending, notified, and failed recommendation rows are operational context only and must not support taste claims.
- Sharp discussion, debate, and drama coverage can be genuine positives for this listener; do not flag them as aversions without explicit negative evidence.
- Separate stable preferences from conditional/contextual ones (mood, episode length, format).
- Preserve some exploration and state uncertainties instead of inventing certainty.
- Every profile field must cite evidence ids from the supplied ledger. Stable, conditional, and saturation claims need at least two independent shows. One explicit not_for_me item may support an aversion. Exploration and uncertainty entries need at least one taste-bearing item.
- Free-form notes on feedback rows are interpretive context to help you understand the structured feedback (or, if there is no good_pick/not_for_me on that row, the only signal). They are not independent evidence: never let a single vivid note carry more weight than one show's worth of evidence toward the independent-show requirement for a claim.
- Do not propose changes to code, prompts, weights, or automation.

DETERMINISTIC STATS:
${JSON.stringify(stats, null, 2)}

EVIDENCE LEDGER:
${evidenceJson}`;
}

function buildCriticPrompt(
  evidenceJson: string,
  stats: unknown,
  draftJson: string,
): string {
  return `Act as a skeptical second-pass reviewer of a podcast taste profile. Return a corrected final profile.

Remove overfitting, unsupported format/genre claims, use of pending/notified/failed operational rows as taste evidence, and claims whose cited ids do not exist. Reduce confidence when evidence is ambiguous. Every field must cite taste-bearing evidence. Stable, conditional, and saturation claims need two independent shows, while one explicit not_for_me item may support an aversion. Exploration and uncertainty need at least one relevant item. Free-form notes are interpretive context, not independent evidence: flag any claim that leans on a note's wording instead of the independent-show count it actually has.

DETERMINISTIC STATS:
${JSON.stringify(stats, null, 2)}

EVIDENCE LEDGER:
${evidenceJson}

DRAFT TO AUDIT:
${draftJson}`;
}
