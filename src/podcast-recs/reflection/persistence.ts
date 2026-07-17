import { Entity } from "@micthiesen/mitools/entities";
import type { PodcastTasteEvidenceData, PodcastTasteProfileData } from "./types.js";

export const PodcastTasteEvidenceEntity = new Entity<
  PodcastTasteEvidenceData,
  ["evidenceId"]
>("podcast-taste-evidence", ["evidenceId"]);

export const PodcastTasteProfileEntity = new Entity<
  PodcastTasteProfileData,
  ["profileId"]
>("podcast-taste-profile", ["profileId"]);

/** Append-only: existing evidence rows are never mutated. */
export function insertPodcastTasteEvidence(
  evidence: PodcastTasteEvidenceData[],
): number {
  let inserted = 0;
  for (const item of evidence) {
    if (PodcastTasteEvidenceEntity.has({ evidenceId: item.evidenceId })) continue;
    PodcastTasteEvidenceEntity.upsert(item);
    inserted++;
  }
  return inserted;
}

export function getAllPodcastTasteEvidence(): PodcastTasteEvidenceData[] {
  return PodcastTasteEvidenceEntity.getAll().sort(
    (a, b) => b.observedAt - a.observedAt,
  );
}

/** Immutable checkpoint: no-op if the profileId already exists. */
export function insertPodcastTasteProfile(profile: PodcastTasteProfileData): boolean {
  if (PodcastTasteProfileEntity.has({ profileId: profile.profileId })) return false;
  PodcastTasteProfileEntity.upsert(profile);
  return true;
}

export function getLatestPodcastTasteProfile(): PodcastTasteProfileData | undefined {
  return PodcastTasteProfileEntity.getAll().sort(
    (a, b) => b.version - a.version || b.generatedAt - a.generatedAt,
  )[0];
}
