import { describe, expect, it } from "vitest";
import { RecommendationStatus } from "../persistence.js";
import { MediaType } from "../types.js";
import {
  deriveRecommendationEvidence,
  deriveWatchEvidence,
  fingerprintEvidence,
} from "./evidence.js";
import {
  formatTasteProfileDigest,
  selectReflectionEvidence,
  validateProfile,
} from "./reflection.js";
import { computeBehavioralStats } from "./stats.js";
import type { TasteEvidenceData, TasteProfileData } from "./types.js";

const NOW = 1_800_000_000_000;

describe("taste evidence", () => {
  it("derives deterministic watch ids and fingerprints independent of order", () => {
    const observations = [
      {
        canonicalId: "tmdb:movie:1" as const,
        item: {
          guid: "tmdb://1",
          title: "Arrival",
          mediaType: MediaType.Movie,
          viewedAt: NOW,
          viewCount: 2,
          completion: 0.98,
        },
      },
      {
        canonicalId: "tmdb:tv:2" as const,
        item: {
          guid: "tmdb://2",
          title: "Severance",
          mediaType: MediaType.Tv,
          viewedAt: NOW - 1,
          viewCount: 1,
          completion: 1,
        },
      },
    ];
    const first = deriveWatchEvidence(observations);
    const second = deriveWatchEvidence(observations);
    expect(second).toEqual(first);
    expect(fingerprintEvidence(first)).toBe(fingerprintEvidence([...first].reverse()));
  });

  it("records recommendation outcome and explicit feedback separately", () => {
    const evidence = deriveRecommendationEvidence([
      {
        recommendationId: "rec-1",
        canonicalId: "tmdb:movie:1",
        tmdbId: 1,
        mediaType: MediaType.Movie,
        title: "Arrival",
        status: RecommendationStatus.Watched,
        runDate: "2027-01-01",
        recommendedAt: NOW - 1000,
        resolvedAt: NOW,
        feedback: "good_pick",
        feedbackAt: NOW + 1,
      },
    ]);
    expect(evidence.map((item) => item.kind)).toEqual([
      "recommendation_outcome",
      "explicit_feedback",
    ]);
    expect(new Set(evidence.map((item) => item.evidenceId)).size).toBe(2);
  });

  it("records pending recommendations so awaiting-outcome stats are complete", () => {
    const evidence = deriveRecommendationEvidence([
      {
        recommendationId: "rec-pending",
        canonicalId: "tmdb:movie:2",
        tmdbId: 2,
        mediaType: MediaType.Movie,
        title: "Pending",
        status: RecommendationStatus.Notified,
        runDate: "2027-01-01",
        recommendedAt: NOW - 1000,
        notifiedAt: NOW,
      },
    ]);
    expect(computeBehavioralStats(evidence).recommendations).toMatchObject({
      total: 1,
      awaitingOutcome: 1,
    });
  });
});

describe("behavioral stats", () => {
  it("deduplicates watch snapshots and uses latest recommendation signals", () => {
    const evidence: TasteEvidenceData[] = [
      watch("watch-old", NOW - 100, 1),
      watch("watch-new", NOW, 3),
      recEvidence("outcome", "recommendation_outcome", {
        recommendationStatus: RecommendationStatus.Watched,
        source: "similar",
      }),
      recEvidence("feedback", "explicit_feedback", {
        feedback: "good_pick",
        source: "similar",
      }),
    ];
    const stats = computeBehavioralStats(evidence);
    expect(stats.completedMovies).toBe(1);
    expect(stats.rewatchedTitles).toBe(1);
    expect(stats.recommendations).toMatchObject({ total: 1, watched: 1 });
    expect(stats.feedback.goodPick).toBe(1);
    expect(stats.sourcePerformance.similar).toMatchObject({
      total: 1,
      watched: 1,
      goodPick: 1,
    });
  });

  it("does not count a partial Plex observation as a completed title", () => {
    const partial = { ...watch("partial", NOW, 1), completion: 0.35 };
    const stats = computeBehavioralStats([partial]);
    expect(stats.completedMovies).toBe(0);
    expect(stats.rewatchedTitles).toBe(0);
  });

  it("keeps failed attempts out of delivered totals and resolves explicit declines", () => {
    const failed = recEvidence("failed", "recommendation_outcome", {
      recommendationId: "rec-failed",
      recommendationStatus: RecommendationStatus.Failed,
      source: "trending",
    });
    const delivered = recEvidence("delivered", "recommendation_outcome", {
      recommendationId: "rec-declined",
      recommendationStatus: RecommendationStatus.Notified,
      source: "similar",
    });
    const declined = recEvidence("declined", "explicit_feedback", {
      recommendationId: "rec-declined",
      feedback: "not_for_me",
      source: "similar",
    });
    const stats = computeBehavioralStats([failed, delivered, declined]);
    expect(stats.recommendations).toMatchObject({
      total: 1,
      failed: 1,
      awaitingOutcome: 0,
    });
    expect(stats.sourcePerformance).toEqual({
      similar: { total: 1, watched: 0, goodPick: 0, notForMe: 1 },
    });
  });
});

describe("profile guardrails", () => {
  it("rejects unsupported claims while allowing one explicit negative aversion", () => {
    const positiveA = watch("positive-a", NOW, 1);
    const positiveB = {
      ...watch("positive-b", NOW - 1, 1),
      canonicalId: "tmdb:movie:2" as const,
    };
    const negative = recEvidence("negative", "explicit_feedback", {
      feedback: "not_for_me",
    });
    const validated = validateProfile(
      {
        stable_preferences: [
          {
            claim: "Likes thoughtful science fiction",
            confidence: 0.8,
            evidence_ids: [positiveA.evidenceId, positiveB.evidenceId],
          },
          {
            claim: "Likes musicals",
            confidence: 0.9,
            evidence_ids: ["made-up"],
          },
        ],
        conditional_preferences: [],
        aversions: [
          {
            claim: "Avoid this pattern",
            confidence: 0.7,
            evidence_ids: [negative.evidenceId],
          },
        ],
        current_saturation: [],
        exploration_targets: [
          {
            claim: "Nearby speculative drama",
            confidence: 0.6,
            evidence_ids: [positiveA.evidenceId],
          },
        ],
        uncertainties: [
          {
            claim: "Comedy preferences",
            confidence: 0.5,
            evidence_ids: [positiveB.evidenceId],
          },
        ],
        commitment_preferences: {
          movies: {
            preference: "positive",
            confidence: 0.8,
            evidence_ids: [positiveA.evidenceId, positiveB.evidenceId],
          },
          limited_series: {
            preference: "uncertain",
            confidence: 0.5,
            evidence_ids: [positiveA.evidenceId, positiveB.evidenceId],
          },
          long_series: {
            preference: "negative",
            confidence: 0.7,
            evidence_ids: [positiveA.evidenceId, positiveB.evidenceId],
          },
        },
      },
      [positiveA, positiveB, negative],
    );
    expect(validated.rejectedClaims).toBe(1);
    expect(validated.profile.stablePreferences).toHaveLength(1);
    expect(validated.profile.aversions).toHaveLength(1);
  });

  it("prioritizes explicit feedback within the evidence prompt bound", () => {
    const selected = selectReflectionEvidence(
      [
        watch("watch", NOW, 1),
        recEvidence("outcome", "recommendation_outcome", {
          recommendationStatus: RecommendationStatus.Watched,
        }),
        recEvidence("feedback", "explicit_feedback", { feedback: "good_pick" }),
      ],
      2,
    );
    expect(selected.map((item) => item.kind)).toEqual([
      "explicit_feedback",
      "recommendation_outcome",
    ]);
  });

  it("formats a compact profile digest for recommendation prompts", () => {
    const profile: TasteProfileData = {
      profileId: "v1:test",
      version: 1,
      generatedAt: NOW,
      evidenceFingerprint: "test",
      evidenceCount: 2,
      modelId: "test:model",
      promptVersion: "test",
      summary: "Likes precise speculative stories.",
      stablePreferences: [
        {
          claim: "Precise speculative stories",
          confidence: 0.8,
          evidenceIds: ["a", "b"],
        },
      ],
      conditionalPreferences: [],
      aversions: [],
      currentSaturation: [],
      explorationTargets: [
        {
          claim: "International science fiction",
          confidence: 0.7,
          evidenceIds: ["a"],
        },
      ],
      uncertainties: [],
      commitmentPreferences: {
        movies: { preference: "positive", confidence: 0.8, evidenceIds: ["a", "b"] },
        limitedSeries: {
          preference: "neutral",
          confidence: 0.5,
          evidenceIds: ["a", "b"],
        },
        longSeries: {
          preference: "uncertain",
          confidence: 0.3,
          evidenceIds: ["a", "b"],
        },
      },
      stats: {
        completedMovies: 2,
        completedSeries: 0,
        rewatchedTitles: 0,
        recommendations: {
          total: 0,
          watched: 0,
          abandoned: 0,
          ignored: 0,
          failed: 0,
          awaitingOutcome: 0,
        },
        feedback: { goodPick: 0, notForMe: 0, alreadyWatched: 0 },
        sourcePerformance: {},
      },
    };
    const digest = formatTasteProfileDigest(profile);
    expect(digest).toContain("Reflective taste profile v1");
    expect(digest).toContain("International science fiction");
  });
});

function watch(
  evidenceId: string,
  observedAt: number,
  viewCount: number,
): TasteEvidenceData {
  return {
    evidenceId,
    kind: "plex_watch",
    canonicalId: "tmdb:movie:1",
    title: "Arrival",
    mediaType: MediaType.Movie,
    observedAt,
    viewCount,
    completion: 1,
  };
}

function recEvidence(
  evidenceId: string,
  kind: TasteEvidenceData["kind"],
  overrides: Partial<TasteEvidenceData>,
): TasteEvidenceData {
  return {
    evidenceId,
    kind,
    canonicalId: "tmdb:movie:1",
    title: "Arrival",
    mediaType: MediaType.Movie,
    observedAt: NOW,
    recommendationId: "rec-1",
    recommendedAt: NOW - 60 * 60 * 1000,
    ...overrides,
  };
}
