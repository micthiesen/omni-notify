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
  feedback?: RecommendationFeedback;
  feedbackAt?: number;
  /** True when this was the backup candidate promoted after already_exists. */
  wasBackup?: boolean;
};

export const RecommendationEntity = new Entity<
  RecommendationData,
  ["recommendationId"]
>("recs-recommendation-attempt", ["recommendationId"]);

type LegacyRecommendationData = Omit<RecommendationData, "recommendationId">;
const LegacyRecommendationEntity = new Entity<
  LegacyRecommendationData,
  ["canonicalId"]
>("recs-recommendation", ["canonicalId"]);

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

/** Idempotently preserve pre-attempt recommendation rows under the new key. */
export function migrateLegacyRecommendations(): number {
  let migrated = 0;
  for (const legacy of LegacyRecommendationEntity.getAll()) {
    const recommendationId = `legacy:${legacy.canonicalId}:${legacy.recommendedAt}`;
    RecommendationEntity.upsert(
      normalizeLegacyRecommendation(legacy, recommendationId),
    );
    LegacyRecommendationEntity.delete({ canonicalId: legacy.canonicalId });
    migrated++;
  }
  return migrated;
}

export function normalizeLegacyRecommendation(
  legacy: LegacyRecommendationData,
  recommendationId: string,
): RecommendationData {
  const legacyResult = legacy.watchlistResult as string | undefined;
  return {
    ...legacy,
    recommendationId,
    watchlistResult: legacyResult === "skipped" ? "error" : legacy.watchlistResult,
  };
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

export function setRecommendationFeedback(
  recommendationId: string,
  feedback: RecommendationFeedback,
): RecommendationData | undefined {
  const rec = RecommendationEntity.get({ recommendationId });
  if (!rec) return undefined;
  RecommendationEntity.patch(
    { recommendationId },
    { feedback, feedbackAt: Date.now() },
  );
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
      if (!rec.feedback || seen.has(rec.canonicalId)) return false;
      seen.add(rec.canonicalId);
      return true;
    })
    .slice(0, 30);
  if (records.length === 0) return "No explicit recommendation feedback yet.";

  const good = records.filter((rec) => rec.feedback === "good_pick");
  const bad = records.filter((rec) => rec.feedback === "not_for_me");
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
  return lines.join("\n");
}

function formatFeedbackTitle(rec: RecommendationData): string {
  return `${rec.title}${rec.year ? ` (${rec.year})` : ""} [${rec.mediaType}]`;
}
