import { Entity } from "@micthiesen/mitools/entities";
import type { MediaType } from "./types.js";

export enum RecommendationStatus {
  /** Row written, watchlist add + notification not yet confirmed. */
  Pending = "pending",
  /** Notification sent; awaiting an outcome. */
  Notified = "notified",
  /** Watched past the completion threshold. */
  Watched = "watched",
  /** Started but bailed, or removed from the watchlist unwatched. */
  Abandoned = "abandoned",
  /** No engagement within the ignore window. */
  Ignored = "ignored",
  /** Run died between the pending write and notification; reconciled later. */
  Failed = "failed",
}

export type WatchlistWriteResult = "added" | "already_exists" | "skipped" | "error";

export type RecommendationData = {
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
  /** Local date (YYYY-MM-DD) of the run that produced this recommendation. */
  runDate: string;
  recommendedAt: number;
  notifiedAt?: number;
  /** When a terminal outcome (watched/abandoned/ignored) was assigned. */
  resolvedAt?: number;
  watchlistResult?: WatchlistWriteResult;
  /** True when this was the backup candidate promoted after already_exists. */
  wasBackup?: boolean;
};

export const RecommendationEntity = new Entity<RecommendationData, ["canonicalId"]>(
  "recs-recommendation",
  ["canonicalId"],
);

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

const COOLDOWN_MS = 180 * 24 * 60 * 60 * 1000;

/**
 * Canonical ids that must not be recommended (again) right now: everything
 * recommended within the cooldown window, plus terminal negative/positive
 * outcomes which are excluded permanently.
 */
export function getExcludedCanonicalIds(now: number): Set<string> {
  const excluded = new Set<string>();
  for (const rec of RecommendationEntity.getAll()) {
    const permanent =
      rec.status === RecommendationStatus.Watched ||
      rec.status === RecommendationStatus.Abandoned;
    if (permanent || now - rec.recommendedAt < COOLDOWN_MS) {
      excluded.add(rec.canonicalId);
    }
  }
  return excluded;
}

/** Recommendations still awaiting an outcome label. */
export function getOpenRecommendations(): RecommendationData[] {
  return RecommendationEntity.getAll().filter(
    (r) =>
      r.status === RecommendationStatus.Notified ||
      r.status === RecommendationStatus.Pending,
  );
}
