import type { LanguageModel } from "ai";
import type {
  RecommendationData,
  RecommendationFeedback,
  RecommendationStatus,
} from "../persistence.js";
import type { CanonicalId, MediaType, WatchedItem } from "../types.js";

export type TasteEvidenceKind =
  | "plex_watch"
  | "recommendation_outcome"
  | "explicit_feedback";

/**
 * Append-only observation used to explain every derived taste claim. The
 * evidence id is deterministic, so polling the same upstream state is safe.
 */
export interface TasteEvidenceData {
  evidenceId: string;
  kind: TasteEvidenceKind;
  canonicalId: CanonicalId;
  title: string;
  year?: number;
  mediaType: MediaType;
  observedAt: number;
  viewCount?: number;
  completion?: number;
  recommendationId?: string;
  recommendationStatus?: RecommendationStatus;
  feedback?: RecommendationFeedback;
  recommendedAt?: number;
  startedAt?: number;
  source?: string;
  genres?: string[];
  runtimeMinutes?: number;
  seasonCount?: number;
  episodeCount?: number;
  seriesStatus?: string;
  originalLanguage?: string;
  originCountries?: string[];
  creators?: string[];
  cast?: string[];
  keywords?: string[];
  certification?: string;
}

export interface CanonicalWatchObservation {
  canonicalId: CanonicalId;
  item: WatchedItem;
  metadata?: {
    genres?: string[];
    runtimeMinutes?: number;
    seasonCount?: number;
    episodeCount?: number;
    seriesStatus?: string;
    originalLanguage?: string;
    originCountries?: string[];
    creators?: string[];
    cast?: string[];
    keywords?: string[];
    certification?: string;
  };
}

export interface BehavioralStats {
  completedMovies: number;
  completedSeries: number;
  rewatchedTitles: number;
  recommendations: {
    total: number;
    watched: number;
    abandoned: number;
    ignored: number;
    failed: number;
    awaitingOutcome: number;
  };
  feedback: {
    goodPick: number;
    notForMe: number;
    alreadyWatched: number;
  };
  averageHoursToStart?: number;
  sourcePerformance: Record<
    string,
    { total: number; watched: number; goodPick: number; notForMe: number }
  >;
}

export interface TasteClaim {
  claim: string;
  confidence: number;
  evidenceIds: string[];
}

export type CommitmentPreference = "positive" | "neutral" | "negative" | "uncertain";

export interface CommitmentAssessment {
  preference: CommitmentPreference;
  confidence: number;
  evidenceIds: string[];
}

export interface TasteProfileContent {
  summary: string;
  stablePreferences: TasteClaim[];
  conditionalPreferences: TasteClaim[];
  aversions: TasteClaim[];
  currentSaturation: TasteClaim[];
  explorationTargets: TasteClaim[];
  uncertainties: TasteClaim[];
  commitmentPreferences: {
    movies: CommitmentAssessment;
    limitedSeries: CommitmentAssessment;
    longSeries: CommitmentAssessment;
  };
}

export interface TasteProfileData extends TasteProfileContent {
  profileId: string;
  version: number;
  generatedAt: number;
  evidenceFingerprint: string;
  evidenceCount: number;
  modelId: string;
  promptVersion: string;
  stats: BehavioralStats;
}

export interface TasteReflectionInput {
  watched: CanonicalWatchObservation[];
  recommendations: RecommendationData[];
  model: LanguageModel;
  modelId: string;
  now?: number;
  /** Hard prompt bound after deterministic evidence prioritization. */
  maxEvidence?: number;
}

export type TasteReflectionResult =
  | {
      status: "unchanged";
      profile: TasteProfileData;
      insertedEvidence: number;
    }
  | {
      status: "created";
      profile: TasteProfileData;
      insertedEvidence: number;
      rejectedClaims: number;
    }
  | {
      status: "insufficient_evidence";
      insertedEvidence: number;
    };
