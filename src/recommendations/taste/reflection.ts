import { generateText, Output } from "ai";
import { z } from "zod";
import { toDateStamp } from "../../utils/dates.js";
import { RecommendationStatus } from "../persistence.js";
import { MediaType } from "../types.js";
import {
  deriveRecommendationEvidence,
  deriveWatchEvidence,
  fingerprintEvidence,
} from "./evidence.js";
import {
  getAllTasteEvidence,
  getLatestTasteProfile,
  insertTasteEvidence,
  insertTasteProfile,
} from "./persistence.js";
import { computeBehavioralStats } from "./stats.js";
import type {
  TasteClaim,
  TasteEvidenceData,
  TasteProfileContent,
  TasteProfileData,
  TasteReflectionInput,
  TasteReflectionResult,
} from "./types.js";

export const TASTE_PROMPT_VERSION = "taste-reflection-v1";
const DEFAULT_MAX_EVIDENCE = 160;

const claimSchema = z.object({
  claim: z.string().min(1).max(240),
  confidence: z.number().min(0).max(1),
  evidence_ids: z.array(z.string()).max(12),
});

const commitmentSchema = z.object({
  preference: z.enum(["positive", "neutral", "negative", "uncertain"]),
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
  commitment_preferences: z.object({
    movies: commitmentSchema,
    limited_series: commitmentSchema,
    long_series: commitmentSchema,
  }),
});

type RawProfile = z.infer<typeof profileSchema>;

/**
 * Append current observations, checkpoint on their fingerprint, then run a
 * draft and a skeptical revision. Model output is constrained and claims that
 * cite missing or inadequate evidence are removed before persistence.
 */
export async function runTasteReflection(
  input: TasteReflectionInput,
): Promise<TasteReflectionResult> {
  const incoming = [
    ...deriveWatchEvidence(input.watched),
    ...deriveRecommendationEvidence(input.recommendations),
  ];
  const insertedEvidence = insertTasteEvidence(incoming);
  const allEvidence = getAllTasteEvidence();
  if (allEvidence.length === 0) {
    return { status: "insufficient_evidence", insertedEvidence };
  }

  const evidenceFingerprint = fingerprintEvidence(allEvidence);
  const latest = getLatestTasteProfile();
  if (latest?.evidenceFingerprint === evidenceFingerprint) {
    return { status: "unchanged", profile: latest, insertedEvidence };
  }

  const boundedEvidence = selectReflectionEvidence(
    allEvidence,
    input.maxEvidence ?? DEFAULT_MAX_EVIDENCE,
  );
  const stats = computeBehavioralStats(allEvidence);
  const evidenceJson = JSON.stringify(boundedEvidence.map(compactEvidence), null, 2);
  const draftResult = await generateText({
    model: input.model,
    output: Output.object({ schema: profileSchema }),
    prompt: buildDraftPrompt(evidenceJson, stats),
  });
  if (!draftResult.output) throw new Error("Taste reflection returned no draft");

  const finalResult = await generateText({
    model: input.model,
    output: Output.object({ schema: profileSchema }),
    prompt: buildCriticPrompt(
      evidenceJson,
      stats,
      JSON.stringify(draftResult.output, null, 2),
    ),
  });
  if (!finalResult.output) throw new Error("Taste reflection returned no revision");

  const validated = validateProfile(finalResult.output, allEvidence);
  const generatedAt = input.now ?? Date.now();
  const version = (latest?.version ?? 0) + 1;
  const profile: TasteProfileData = {
    ...validated.profile,
    profileId: `v${version}:${evidenceFingerprint}`,
    version,
    generatedAt,
    evidenceFingerprint,
    evidenceCount: allEvidence.length,
    modelId: input.modelId,
    promptVersion: TASTE_PROMPT_VERSION,
    stats,
  };
  insertTasteProfile(profile);
  return {
    status: "created",
    profile,
    insertedEvidence,
    rejectedClaims: validated.rejectedClaims,
  };
}

/** Prefer direct feedback, then recommendation outcomes, then recent watches. */
export function selectReflectionEvidence(
  evidence: TasteEvidenceData[],
  limit: number,
): TasteEvidenceData[] {
  if (limit <= 0) return [];
  const weight = (item: TasteEvidenceData) => {
    if (item.kind === "explicit_feedback") return 4;
    if (
      item.recommendationStatus === RecommendationStatus.Watched ||
      item.recommendationStatus === RecommendationStatus.Abandoned ||
      item.recommendationStatus === RecommendationStatus.Ignored
    )
      return 3;
    if (item.kind === "plex_watch") return 2;
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

export function validateProfile(
  raw: RawProfile,
  evidence: TasteEvidenceData[],
): { profile: TasteProfileContent; rejectedClaims: number } {
  const byId = new Map(evidence.map((item) => [item.evidenceId, item]));
  let rejectedClaims = 0;
  const validateClaims = (
    claims: RawProfile["stable_preferences"],
    minimumTitles: number,
    allowSingleExplicitAversion = false,
  ): TasteClaim[] =>
    claims.flatMap((claim) => {
      const evidenceIds = [...new Set(claim.evidence_ids)].filter((id) => {
        const item = byId.get(id);
        return item !== undefined && isTasteBearingEvidence(item);
      });
      const hasExplicitNegative = evidenceIds.some(
        (id) => byId.get(id)?.feedback === "not_for_me",
      );
      const independentTitles = new Set(
        evidenceIds.map((id) => byId.get(id)?.canonicalId).filter(Boolean),
      ).size;
      const enough =
        independentTitles >= minimumTitles ||
        (allowSingleExplicitAversion && hasExplicitNegative);
      if (!enough) {
        rejectedClaims++;
        return [];
      }
      return [{ claim: claim.claim, confidence: claim.confidence, evidenceIds }];
    });

  const validateCommitment = (
    assessment: RawProfile["commitment_preferences"]["movies"],
  ) => {
    const evidenceIds = [...new Set(assessment.evidence_ids)].filter((id) => {
      const item = byId.get(id);
      return item !== undefined && isTasteBearingEvidence(item);
    });
    const independentTitles = new Set(
      evidenceIds.map((id) => byId.get(id)?.canonicalId).filter(Boolean),
    ).size;
    if (independentTitles < 2) {
      rejectedClaims++;
      return { preference: "uncertain" as const, confidence: 0, evidenceIds: [] };
    }
    return {
      preference: assessment.preference,
      confidence: assessment.confidence,
      evidenceIds,
    };
  };

  const stablePreferences = validateClaims(raw.stable_preferences, 2);
  const conditionalPreferences = validateClaims(raw.conditional_preferences, 2);
  const aversions = validateClaims(raw.aversions, 2, true);
  const currentSaturation = validateClaims(raw.current_saturation, 2);
  const explorationTargets = validateClaims(raw.exploration_targets, 1);
  const uncertainties = validateClaims(raw.uncertainties, 1);
  const summaryParts = [
    stablePreferences.length > 0
      ? `Evidence-backed preferences: ${stablePreferences.map((item) => item.claim).join("; ")}.`
      : "Taste evidence is still limited.",
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
      commitmentPreferences: {
        movies: validateCommitment(raw.commitment_preferences.movies),
        limitedSeries: validateCommitment(raw.commitment_preferences.limited_series),
        longSeries: validateCommitment(raw.commitment_preferences.long_series),
      },
    },
    rejectedClaims,
  };
}

function isTasteBearingEvidence(item: TasteEvidenceData): boolean {
  if (item.kind === "explicit_feedback") {
    return item.feedback === "good_pick" || item.feedback === "not_for_me";
  }
  if (item.kind === "recommendation_outcome") {
    return (
      item.recommendationStatus === RecommendationStatus.Watched ||
      item.recommendationStatus === RecommendationStatus.Abandoned ||
      item.recommendationStatus === RecommendationStatus.Ignored
    );
  }
  return item.completion === undefined
    ? item.mediaType === MediaType.Movie && (item.viewCount ?? 0) >= 1
    : item.completion >= 0.8;
}

export function formatTasteProfileDigest(
  profile: TasteProfileData | undefined = getLatestTasteProfile(),
): string {
  if (!profile) return "No reflective taste profile is available yet.";
  const claimLines = (label: string, claims: TasteClaim[]) =>
    claims.length > 0
      ? `${label}: ${claims.map((item) => item.claim).join("; ")}`
      : undefined;
  return [
    `Reflective taste profile v${profile.version}: ${profile.summary}`,
    claimLines("Stable preferences", profile.stablePreferences),
    claimLines("Conditional preferences", profile.conditionalPreferences),
    claimLines("Aversions", profile.aversions),
    claimLines("Current saturation", profile.currentSaturation),
    claimLines("Exploration targets", profile.explorationTargets),
    claimLines("Uncertainties", profile.uncertainties),
    `Commitment fit: movies=${profile.commitmentPreferences.movies.preference}, limited-series=${profile.commitmentPreferences.limitedSeries.preference}, long-series=${profile.commitmentPreferences.longSeries.preference}`,
  ]
    .filter(Boolean)
    .join("\n");
}

function compactEvidence(item: TasteEvidenceData): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries({
      id: item.evidenceId,
      kind: item.kind,
      title: item.title,
      year: item.year,
      media_type: item.mediaType,
      observed_at: toDateStamp(item.observedAt),
      view_count: item.viewCount,
      completion: item.completion,
      recommendation_status: item.recommendationStatus,
      feedback: item.feedback,
      source: item.source,
      genres: item.genres,
      runtime_minutes: item.runtimeMinutes,
      season_count: item.seasonCount,
      episode_count: item.episodeCount,
      series_status: item.seriesStatus,
      original_language: item.originalLanguage,
      origin_countries: item.originCountries,
      creators: item.creators,
      cast: item.cast,
      keywords: item.keywords,
      certification: item.certification,
    }).filter(([, value]) => value !== undefined),
  );
}

function buildDraftPrompt(evidenceJson: string, stats: unknown): string {
  return `Build a conservative, useful taste profile for one person's movie and TV recommendation system.

Rules:
- Infer preferences from demonstrated behavior, not popularity or stereotypes.
- Rewatches and explicit feedback are strongest. A completed watch is positive-but-ambiguous. Ignored is weak evidence. "already_watched" is not negative taste evidence. Pending, notified, and failed recommendation rows are operational context only and must not support taste claims.
- Separate stable preferences from conditional/contextual ones.
- Preserve some exploration and state uncertainties instead of inventing certainty.
- Every profile field must cite evidence ids from the supplied ledger, including saturation, exploration, uncertainty, and commitment assessments. Stable, conditional, saturation, and commitment claims need at least two independent titles. One explicit not_for_me item may support an aversion. Exploration and uncertainty entries need at least one taste-bearing item.
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
  return `Act as a skeptical second-pass reviewer of a movie/TV taste profile. Return a corrected final profile.

Remove overfitting, unsupported genre claims, accidental treatment of "already_watched" as dislike, use of pending/notified/failed operational rows as taste evidence, and claims whose cited ids do not exist. Reduce confidence when evidence is ambiguous. Every field must cite taste-bearing evidence, including saturation, exploration, uncertainty, and commitment assessments. Stable, conditional, saturation, and commitment claims need two independent titles, while one explicit not_for_me item may support an aversion. Exploration and uncertainty need at least one relevant item.

DETERMINISTIC STATS:
${JSON.stringify(stats, null, 2)}

EVIDENCE LEDGER:
${evidenceJson}

DRAFT TO AUDIT:
${draftJson}`;
}
