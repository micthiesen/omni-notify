import { describe, expect, it } from "vitest";
import { PodcastRecommendationStatus } from "../persistence.js";
import {
  deriveListenEvidence,
  deriveRecommendationEvidence,
  fingerprintEvidence,
  normalizeShowKey,
} from "./evidence.js";
import {
  formatPodcastTasteProfileDigest,
  selectPodcastReflectionEvidence,
  validatePodcastProfile,
} from "./reflection.js";
import { computePodcastBehavioralStats } from "./stats.js";
import type { PodcastTasteEvidenceData, PodcastTasteProfileData } from "./types.js";

function makeEvidence(
  overrides: Partial<PodcastTasteEvidenceData>,
): PodcastTasteEvidenceData {
  return {
    evidenceId: "listen:abc",
    kind: "listen",
    showKey: "search engine",
    showTitle: "Search Engine",
    episodeTitle: "What is money?",
    observedAt: 1_000,
    completion: 1,
    ...overrides,
  };
}

describe("normalizeShowKey", () => {
  it("lowercases and trims", () => {
    expect(normalizeShowKey("  Search Engine ")).toBe("search engine");
  });
});

describe("deriveListenEvidence", () => {
  const listen = {
    showTitle: "Search Engine",
    episodeTitle: "What is money?",
    episodeGuid: "guid-1",
    listenedAt: 1_000,
    completion: 0.92,
    starred: true,
  };

  it("is deterministic for identical observations", () => {
    const [a] = deriveListenEvidence([listen]);
    const [b] = deriveListenEvidence([{ ...listen }]);
    expect(a.evidenceId).toBe(b.evidenceId);
    expect(a.kind).toBe("listen");
    expect(a.showKey).toBe("search engine");
  });

  it("changes id when the observation changes", () => {
    const [a] = deriveListenEvidence([listen]);
    const [b] = deriveListenEvidence([{ ...listen, completion: 0.5 }]);
    expect(a.evidenceId).not.toBe(b.evidenceId);
  });
});

describe("deriveRecommendationEvidence", () => {
  const rec = {
    recommendationId: "rec-1",
    episodeId: "itunes:1#guid",
    showId: "itunes:1",
    showTitle: "Blocked and Reported",
    episodeTitle: "Episode 100",
    feedUrl: "https://example.com/feed",
    episodeGuid: "guid-100",
    publishedAt: 900,
    status: PodcastRecommendationStatus.Listened,
    runDate: "2026-07-01",
    recommendedAt: 950,
    notifiedAt: 960,
    resolvedAt: 990,
  };

  it("emits an outcome row, and a feedback row only when feedback exists", () => {
    expect(deriveRecommendationEvidence([rec])).toHaveLength(1);
    const withFeedback = deriveRecommendationEvidence([
      { ...rec, feedback: "good_pick" as const, feedbackAt: 995 },
    ]);
    expect(withFeedback).toHaveLength(2);
    expect(withFeedback[1].kind).toBe("explicit_feedback");
    expect(withFeedback[1].feedback).toBe("good_pick");
  });
});

describe("fingerprintEvidence", () => {
  it("is order-independent", () => {
    const a = makeEvidence({ evidenceId: "listen:a" });
    const b = makeEvidence({ evidenceId: "listen:b", showKey: "other show" });
    expect(fingerprintEvidence([a, b])).toBe(fingerprintEvidence([b, a]));
  });

  it("changes when evidence changes", () => {
    const a = makeEvidence({ evidenceId: "listen:a" });
    expect(fingerprintEvidence([a])).not.toBe(
      fingerprintEvidence([{ ...a, completion: 0.2 }]),
    );
  });
});

describe("selectPodcastReflectionEvidence", () => {
  it("prefers feedback, then delivered outcomes, then listens", () => {
    const listen = makeEvidence({ evidenceId: "listen:a", observedAt: 3_000 });
    const outcome = makeEvidence({
      evidenceId: "recommendation:b",
      kind: "recommendation_outcome",
      recommendationStatus: PodcastRecommendationStatus.Listened,
      observedAt: 2_000,
    });
    const feedback = makeEvidence({
      evidenceId: "recommendation:c",
      kind: "explicit_feedback",
      feedback: "not_for_me",
      observedAt: 1_000,
    });
    const selected = selectPodcastReflectionEvidence([listen, outcome, feedback], 2);
    expect(selected.map((item) => item.evidenceId)).toEqual([
      "recommendation:c",
      "recommendation:b",
    ]);
  });
});

describe("computePodcastBehavioralStats", () => {
  it("counts listens, outcomes, and feedback with dedup", () => {
    const stats = computePodcastBehavioralStats([
      makeEvidence({ evidenceId: "l1", completion: 1, starred: true }),
      // Newer observation of the same episode wins.
      makeEvidence({ evidenceId: "l2", completion: 0.1, observedAt: 2_000 }),
      makeEvidence({
        evidenceId: "l3",
        showKey: "other",
        showTitle: "Other",
        episodeTitle: "Ep 2",
        completion: undefined,
      }),
      makeEvidence({
        evidenceId: "o1",
        kind: "recommendation_outcome",
        recommendationId: "rec-1",
        recommendationStatus: PodcastRecommendationStatus.Abandoned,
      }),
      makeEvidence({
        evidenceId: "f1",
        kind: "explicit_feedback",
        recommendationId: "rec-1",
        feedback: "not_for_me",
      }),
    ]);
    expect(stats.startedEpisodes).toBe(2);
    // l2 (10% completion) superseded l1, so only the no-completion listen counts.
    expect(stats.listenedEpisodes).toBe(1);
    expect(stats.distinctShows).toBe(2);
    expect(stats.recommendations.abandoned).toBe(1);
    expect(stats.feedback.notForMe).toBe(1);
  });
});

describe("validatePodcastProfile", () => {
  const evidence = [
    makeEvidence({ evidenceId: "l1", showKey: "show a", completion: 1 }),
    makeEvidence({ evidenceId: "l2", showKey: "show b", completion: 0.9 }),
    makeEvidence({
      evidenceId: "f1",
      kind: "explicit_feedback",
      showKey: "show c",
      feedback: "not_for_me",
    }),
    // Not taste-bearing: a shallow, unstarred listen.
    makeEvidence({ evidenceId: "weak", showKey: "show d", completion: 0.1 }),
  ];
  const emptyProfile = {
    stable_preferences: [],
    conditional_preferences: [],
    aversions: [],
    current_saturation: [],
    exploration_targets: [],
    uncertainties: [],
  };

  it("keeps claims with two independent shows", () => {
    const result = validatePodcastProfile(
      {
        ...emptyProfile,
        stable_preferences: [
          {
            claim: "Likes deep-dive interviews",
            confidence: 0.8,
            evidence_ids: ["l1", "l2"],
          },
        ],
      },
      evidence,
    );
    expect(result.profile.stablePreferences).toHaveLength(1);
    expect(result.rejectedClaims).toBe(0);
  });

  it("drops claims backed by one show or weak evidence", () => {
    const result = validatePodcastProfile(
      {
        ...emptyProfile,
        stable_preferences: [
          { claim: "One-show claim", confidence: 0.9, evidence_ids: ["l1"] },
          { claim: "Weak-evidence claim", confidence: 0.9, evidence_ids: ["weak"] },
          { claim: "Phantom ids", confidence: 0.9, evidence_ids: ["nope", "nada"] },
        ],
      },
      evidence,
    );
    expect(result.profile.stablePreferences).toHaveLength(0);
    expect(result.rejectedClaims).toBe(3);
  });

  it("allows an aversion backed by a single explicit not_for_me", () => {
    const result = validatePodcastProfile(
      {
        ...emptyProfile,
        aversions: [
          { claim: "Not into true crime", confidence: 0.7, evidence_ids: ["f1"] },
        ],
      },
      evidence,
    );
    expect(result.profile.aversions).toHaveLength(1);
  });
});

describe("formatPodcastTasteProfileDigest", () => {
  it("formats a profile with claims", () => {
    const profile: PodcastTasteProfileData = {
      profileId: "v2:abc",
      version: 2,
      generatedAt: 1_000,
      evidenceFingerprint: "abc",
      evidenceCount: 10,
      modelId: "openai:gpt-5.6-luna",
      promptVersion: "podcast-taste-reflection-v1",
      summary: "Likes sharp interview shows.",
      stablePreferences: [
        { claim: "Deep-dive interviews", confidence: 0.8, evidenceIds: ["l1"] },
      ],
      conditionalPreferences: [],
      aversions: [],
      currentSaturation: [],
      explorationTargets: [],
      uncertainties: [],
      stats: {
        listenedEpisodes: 5,
        startedEpisodes: 6,
        starredEpisodes: 1,
        distinctShows: 4,
        recommendations: {
          total: 3,
          listened: 1,
          abandoned: 0,
          ignored: 1,
          failed: 0,
          awaitingOutcome: 1,
        },
        feedback: { goodPick: 1, notForMe: 0 },
      },
    };
    const digest = formatPodcastTasteProfileDigest(profile);
    expect(digest).toContain("Reflective podcast taste profile v2");
    expect(digest).toContain("Stable preferences: Deep-dive interviews");
  });
});
