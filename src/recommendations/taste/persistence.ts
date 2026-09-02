import { Entity } from "@micthiesen/mitools/entities";
import { Effect } from "effect";
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
export const insertTasteEvidence = Effect.fn("Taste.insertEvidence")(function* (
  evidence: TasteEvidenceData[],
) {
  let inserted = 0;
  for (const item of evidence) {
    if (yield* TasteEvidenceEntity.has({ evidenceId: item.evidenceId })) continue;
    yield* TasteEvidenceEntity.upsert(item);
    inserted++;
  }
  return inserted;
});

export const getAllTasteEvidence = Effect.fn("Taste.getAllEvidence")(function* () {
  return (yield* TasteEvidenceEntity.getAll()).sort(
    (a, b) => b.observedAt - a.observedAt,
  );
});

/** Profile ids are immutable checkpoints, just like evidence ids. */
export const insertTasteProfile = Effect.fn("Taste.insertProfile")(function* (
  profile: TasteProfileData,
) {
  if (yield* TasteProfileEntity.has({ profileId: profile.profileId })) return false;
  yield* TasteProfileEntity.upsert(profile);
  return true;
});

export const getLatestTasteProfile = Effect.fn("Taste.getLatestProfile")(function* () {
  return (yield* TasteProfileEntity.getAll()).sort(
    (a, b) => b.version - a.version || b.generatedAt - a.generatedAt,
  )[0];
});
