import { Entity } from "@micthiesen/mitools/entities";
import type { CandidateSource, MediaType } from "./types.js";

export enum RecommendationStatus {
  /** Row written, acquisition + notification not yet confirmed. */
  Pending = "pending",
  /** Notification sent; awaiting an outcome. */
  Notified = "notified",
  /** Watched past the completion threshold. */
  Watched = "watched",
  /** Started but bailed below the completion threshold. */
  Abandoned = "abandoned",
  /** No engagement within the ignore window. */
  Ignored = "ignored",
  /** Run died between the pending write and notification; reconciled later. */
  Failed = "failed",
}

export type RecommendationFeedback = "good_pick" | "not_for_me" | "already_watched";

export type WatchlistWriteResult = "added" | "already_exists" | "available" | "error";

export type RecommendationData = {
  recommendationId: string;
  canonicalId: string;
  tmdbId: number;
  mediaType: MediaType;
  title: string;
  year?: number;
  posterPath?: string;
  status: RecommendationStatus;
  whyForUser?: string;
  caveats?: string[];
  confidence?: number;
  /** Selection-time evidence retained for later audit and reflection. */
  source?: CandidateSource;
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
  shortlistScores?: {
    tasteMatch: number;
    novelty: number;
    effortFit: number;
    composite: number;
    risks: string[];
  };
  /** Local date (YYYY-MM-DD) of the run that produced this recommendation. */
  runDate: string;
  recommendedAt: number;
  notifiedAt?: number;
  /** First time Plex showed playback or partial progress after delivery. */
  startedAt?: number;
  /** When a terminal outcome (watched/abandoned/ignored) was assigned. */
  resolvedAt?: number;
  watchlistResult?: WatchlistWriteResult;
  /** Sonarr's series URL slug, captured at add time (used for UI deep links). */
  managerSlug?: string;
  feedback?: RecommendationFeedback;
  feedbackAt?: number;
  /** Optional free-form note alongside (or instead of) the binary feedback. */
  feedbackNote?: string;
  /** True when this was the backup candidate promoted after already_exists. */
  wasBackup?: boolean;
};

export const RecommendationEntity = new Entity<
  RecommendationData,
  ["recommendationId"]
>("recs-recommendation-attempt", ["recommendationId"]);

export type IdentityAliasData = {
  guid: string;
  /** Canonical id, or null when resolution failed (cached to avoid re-lookups). */
  canonicalId: string | null;
  confidence: number;
  resolutionPath: "external-id" | "tmdb-find" | "tmdb-search" | "unresolved";
  title: string;
  resolvedAt: number;
};

export const IdentityAliasEntity = new Entity<IdentityAliasData, ["guid"]>(
  "recs-identity-alias",
  ["guid"],
);

export function getAllRecommendations(): RecommendationData[] {
  return RecommendationEntity.getAll().sort(
    (a, b) => b.recommendedAt - a.recommendedAt,
  );
}

export function getRecommendation(
  recommendationId: string,
): RecommendationData | undefined {
  return RecommendationEntity.get({ recommendationId });
}

const COOLDOWN_MS = 180 * 24 * 60 * 60 * 1000;
const FAILED_RETRY_MS = 24 * 60 * 60 * 1000;

/**
 * Canonical ids that must not be recommended (again) right now: everything
 * recommended within the cooldown window, plus terminal negative/positive
 * outcomes which are excluded permanently.
 */
export function getExcludedCanonicalIds(now: number): Set<string> {
  return computeExcludedCanonicalIds(RecommendationEntity.getAll(), now);
}

export function computeExcludedCanonicalIds(
  records: RecommendationData[],
  now: number,
): Set<string> {
  const excluded = new Set<string>();
  const latestFeedback = new Map<string, RecommendationData>();
  for (const rec of records) {
    const permanent =
      rec.status === RecommendationStatus.Watched ||
      rec.status === RecommendationStatus.Abandoned;
    const cooldown =
      rec.status === RecommendationStatus.Failed ? FAILED_RETRY_MS : COOLDOWN_MS;
    if (permanent || now - rec.recommendedAt < cooldown) {
      excluded.add(rec.canonicalId);
    }
    const prior = latestFeedback.get(rec.canonicalId);
    if (
      rec.feedback &&
      (!prior ||
        (rec.feedbackAt ?? rec.recommendedAt) >
          (prior.feedbackAt ?? prior.recommendedAt))
    ) {
      latestFeedback.set(rec.canonicalId, rec);
    }
  }
  for (const rec of latestFeedback.values()) {
    if (rec.feedback === "not_for_me" || rec.feedback === "already_watched") {
      excluded.add(rec.canonicalId);
    }
  }
  return excluded;
}

/** Recommendations still awaiting an outcome label. */
export function getOpenRecommendations(): RecommendationData[] {
  return RecommendationEntity.getAll().filter(
    (r) =>
      (r.status === RecommendationStatus.Notified ||
        r.status === RecommendationStatus.Pending) &&
      r.feedback !== "not_for_me" &&
      r.feedback !== "already_watched",
  );
}

export type RecommendationFeedbackInput = {
  feedback?: RecommendationFeedback;
  note?: string;
};

/**
 * Overloaded so existing callers passing a bare enum keep compiling: the note
 * is purely additive, and either input alone (or both) is a valid update.
 */
export function setRecommendationFeedback(
  recommendationId: string,
  feedback: RecommendationFeedback,
): RecommendationData | undefined;
export function setRecommendationFeedback(
  recommendationId: string,
  input: RecommendationFeedbackInput,
): RecommendationData | undefined;
export function setRecommendationFeedback(
  recommendationId: string,
  input: RecommendationFeedback | RecommendationFeedbackInput,
): RecommendationData | undefined {
  const rec = RecommendationEntity.get({ recommendationId });
  if (!rec) return undefined;
  const { feedback, note } =
    typeof input === "string" ? { feedback: input, note: undefined } : input;
  const patch: Partial<Omit<RecommendationData, "recommendationId">> = {
    feedbackAt: Date.now(),
  };
  if (feedback !== undefined) patch.feedback = feedback;
  if (note !== undefined) patch.feedbackNote = note;
  RecommendationEntity.patch({ recommendationId }, patch);
  return RecommendationEntity.get({ recommendationId });
}

export function formatFeedbackDigest(): string {
  return formatFeedbackDigestFrom(getAllRecommendations());
}

export function formatFeedbackDigestFrom(input: RecommendationData[]): string {
  const seen = new Set<string>();
  const records = [...input]
    .sort(
      (a, b) => (b.feedbackAt ?? b.recommendedAt) - (a.feedbackAt ?? a.recommendedAt),
    )
    .filter((rec) => {
      if ((!rec.feedback && !rec.feedbackNote) || seen.has(rec.canonicalId)) {
        return false;
      }
      seen.add(rec.canonicalId);
      return true;
    })
    .slice(0, 30);
  if (records.length === 0) return "No explicit recommendation feedback yet.";

  const good = records.filter((rec) => rec.feedback === "good_pick");
  const bad = records.filter((rec) => rec.feedback === "not_for_me");
  const noteOnly = records.filter((rec) => !rec.feedback && rec.feedbackNote);
  const lines = ["Explicit recommendation feedback:"];
  if (good.length > 0) {
    lines.push(
      `- Good picks: ${good.map((rec) => formatFeedbackTitle(rec)).join(", ")}`,
    );
  }
  if (bad.length > 0) {
    lines.push(
      `- Not for me: ${bad.map((rec) => formatFeedbackTitle(rec)).join(", ")}`,
    );
  }
  if (noteOnly.length > 0) {
    const notes = noteOnly.map((rec) => formatFeedbackTitle(rec)).join(", ");
    lines.push(`- Notes (no rating): ${notes}`);
  }
  return lines.join("\n");
}

function formatFeedbackTitle(rec: RecommendationData): string {
  const base = `${rec.title}${rec.year ? ` (${rec.year})` : ""} [${rec.mediaType}]`;
  return rec.feedbackNote ? `${base} — note: "${rec.feedbackNote}"` : base;
}
