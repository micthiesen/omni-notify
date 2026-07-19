import type { LanguageModel } from "ai";
import type { ListenedEpisode } from "../account.js";
import type {
  PodcastFeedback,
  PodcastRecommendationData,
  PodcastRecommendationStatus,
} from "../persistence.js";

export type PodcastTasteEvidenceKind =
  /** A playback event from the podcast account's listen history. */
  | "listen"
  /** The passive outcome of a delivered recommendation. */
  | "recommendation_outcome"
  /** Explicit good-pick/not-for-me feedback from the web UI. */
  | "explicit_feedback";

/** Append-only observation row; the show is the independence unit. */
export type PodcastTasteEvidenceData = {
  evidenceId: string;
  kind: PodcastTasteEvidenceKind;
  /** Normalized show title used to count independent shows per claim. */
  showKey: string;
  showTitle: string;
  episodeTitle?: string;
  observedAt: number;
  /** 0-1 fraction listened, when the client reports it. */
  completion?: number;
  starred?: boolean;
  recommendationId?: string;
  recommendationStatus?: PodcastRecommendationStatus;
  feedback?: PodcastFeedback;
  /** Free-form note left alongside (or instead of) the binary feedback. */
  note?: string;
  discoveredVia?: string;
  matchedVoices?: string[];
  durationMinutes?: number;
};

export type PodcastTasteClaim = {
  claim: string;
  confidence: number;
  evidenceIds: string[];
};

export type PodcastBehavioralStats = {
  /** Episodes finished (≥80% completion, or a playback event with no data). */
  listenedEpisodes: number;
  /** Episodes with any playback activity. */
  startedEpisodes: number;
  starredEpisodes: number;
  distinctShows: number;
  recommendations: {
    total: number;
    listened: number;
    abandoned: number;
    ignored: number;
    failed: number;
    awaitingOutcome: number;
  };
  feedback: {
    goodPick: number;
    notForMe: number;
  };
};

export type PodcastTasteProfileContent = {
  summary: string;
  stablePreferences: PodcastTasteClaim[];
  conditionalPreferences: PodcastTasteClaim[];
  aversions: PodcastTasteClaim[];
  currentSaturation: PodcastTasteClaim[];
  explorationTargets: PodcastTasteClaim[];
  uncertainties: PodcastTasteClaim[];
};

export type PodcastTasteProfileData = PodcastTasteProfileContent & {
  /** `v${version}:${evidenceFingerprint}` — immutable checkpoint id. */
  profileId: string;
  version: number;
  generatedAt: number;
  evidenceFingerprint: string;
  evidenceCount: number;
  modelId: string;
  promptVersion: string;
  stats: PodcastBehavioralStats;
};

export interface PodcastTasteReflectionInput {
  listened: ListenedEpisode[];
  recommendations: PodcastRecommendationData[];
  model: LanguageModel;
  modelId: string;
  now?: number;
  /** Hard prompt bound after deterministic evidence prioritization. */
  maxEvidence?: number;
}

export type PodcastTasteReflectionResult =
  | {
      status: "unchanged";
      profile: PodcastTasteProfileData;
      insertedEvidence: number;
    }
  | {
      status: "created";
      profile: PodcastTasteProfileData;
      insertedEvidence: number;
      rejectedClaims: number;
    }
  | {
      status: "insufficient_evidence";
      insertedEvidence: number;
    };
