import { Entity } from "@micthiesen/mitools/entities";
import { Effect } from "effect";
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
export const insertPodcastTasteEvidence = Effect.fn("PodcastTaste.insertEvidence")(
  function* (evidence: PodcastTasteEvidenceData[]) {
    let inserted = 0;
    for (const item of evidence) {
      if (yield* PodcastTasteEvidenceEntity.has({ evidenceId: item.evidenceId }))
        continue;
      yield* PodcastTasteEvidenceEntity.upsert(item);
      inserted++;
    }
    return inserted;
  },
);

export const getAllPodcastTasteEvidence = Effect.fn("PodcastTaste.getAllEvidence")(
  function* () {
    return (yield* PodcastTasteEvidenceEntity.getAll()).sort(
      (a, b) => b.observedAt - a.observedAt,
    );
  },
);

/** Immutable checkpoint: no-op if the profileId already exists. */
export const insertPodcastTasteProfile = Effect.fn("PodcastTaste.insertProfile")(
  function* (profile: PodcastTasteProfileData) {
    if (yield* PodcastTasteProfileEntity.has({ profileId: profile.profileId }))
      return false;
    yield* PodcastTasteProfileEntity.upsert(profile);
    return true;
  },
);

export const getLatestPodcastTasteProfile = Effect.fn("PodcastTaste.getLatestProfile")(
  function* () {
    return (yield* PodcastTasteProfileEntity.getAll()).sort(
      (a, b) => b.version - a.version || b.generatedAt - a.generatedAt,
    )[0];
  },
);
