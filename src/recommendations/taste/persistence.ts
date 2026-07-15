import { Entity } from "@micthiesen/mitools/entities";
import type { TasteEvidenceData, TasteProfileData } from "./types.js";

export const TasteEvidenceEntity = new Entity<TasteEvidenceData, ["evidenceId"]>(
  "recs-taste-evidence",
  ["evidenceId"],
);

export const TasteProfileEntity = new Entity<TasteProfileData, ["profileId"]>(
  "recs-taste-profile",
  ["profileId"],
);

/** Insert new evidence without ever mutating an observation already recorded. */
export function insertTasteEvidence(evidence: TasteEvidenceData[]): number {
  let inserted = 0;
  for (const item of evidence) {
    if (TasteEvidenceEntity.has({ evidenceId: item.evidenceId })) continue;
    TasteEvidenceEntity.upsert(item);
    inserted++;
  }
  return inserted;
}

export function getAllTasteEvidence(): TasteEvidenceData[] {
  return TasteEvidenceEntity.getAll().sort((a, b) => b.observedAt - a.observedAt);
}

/** Profile ids are immutable checkpoints, just like evidence ids. */
export function insertTasteProfile(profile: TasteProfileData): boolean {
  if (TasteProfileEntity.has({ profileId: profile.profileId })) return false;
  TasteProfileEntity.upsert(profile);
  return true;
}

export function getLatestTasteProfile(): TasteProfileData | undefined {
  return TasteProfileEntity.getAll().sort(
    (a, b) => b.version - a.version || b.generatedAt - a.generatedAt,
  )[0];
}
