import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Clock, Effect, Option } from "effect";

// Dedup gate: tracks every delivery submitted to Parcel API
export type SubmittedDeliveryData = {
  trackingNumber: string;
  carrierCode: string;
  description: string;
  submittedAt: number;
  emailId: string;
  /** Missing on legacy rows, which are already terminal submissions. */
  status?: "pending" | "submitted" | "rejected";
  attempts?: number;
};

export const SubmittedDeliveryEntity = new Entity<
  SubmittedDeliveryData,
  ["trackingNumber"]
>("parcel-submitted-delivery", ["trackingNumber"]);

export const hasSubmittedDelivery = Effect.fn("ParcelPersistence.hasSubmitted")(
  function* (trackingNumber: string) {
    const row = Option.getOrUndefined(
      yield* SubmittedDeliveryEntity.get({ trackingNumber }),
    );
    return row !== undefined && row.status !== "pending";
  },
);

export const getDeliverySubmission = Effect.fn("ParcelPersistence.getSubmission")(
  function* (trackingNumber: string) {
    return Option.getOrUndefined(
      yield* SubmittedDeliveryEntity.get({ trackingNumber }),
    );
  },
);

export const getAllTrackingNumbers = Effect.fn(
  "ParcelPersistence.getAllTrackingNumbers",
)(function* () {
  return new Set(
    (yield* SubmittedDeliveryEntity.getAll())
      .filter((delivery) => delivery.status !== "pending")
      .map((delivery) => delivery.trackingNumber),
  );
});

/** Both strings must be at least this long for a containment match. */
const NEAR_DUPLICATE_MIN_LENGTH = 8;

/**
 * Pure: finds a known tracking number that is a near-duplicate of the
 * candidate. Near-duplicate means the strings are equal, or both are at least
 * 8 chars and one contains the other (merchants truncate the same shipment's
 * number differently, e.g. P5253806501 vs P52538065).
 */
export function findNearDuplicateTracking(
  candidate: string,
  knownNumbers: Iterable<string>,
): string | undefined {
  for (const known of knownNumbers) {
    if (known === candidate) return known;
    if (
      known.length >= NEAR_DUPLICATE_MIN_LENGTH &&
      candidate.length >= NEAR_DUPLICATE_MIN_LENGTH &&
      (known.includes(candidate) || candidate.includes(known))
    ) {
      return known;
    }
  }
  return undefined;
}

export const recordSubmittedDelivery = Effect.fn("ParcelPersistence.recordSubmitted")(
  function* (data: SubmittedDeliveryData) {
    yield* SubmittedDeliveryEntity.upsert(data);
  },
);

/** Persist intent before calling Parcel so an interrupted request is replayable. */
export const reserveDeliverySubmission = Effect.fn("ParcelPersistence.reserve")(
  function* (data: Omit<SubmittedDeliveryData, "status" | "attempts">) {
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    return yield* docstore.transaction("reserve Parcel submission", (tx) => {
      const pk = SubmittedDeliveryEntity.getPk({
        trackingNumber: data.trackingNumber,
      });
      const stored = tx.getRawRow(pk, now);
      const prior = stored ? decodeDoc<SubmittedDeliveryData>(stored.data) : undefined;
      const row: SubmittedDeliveryData = {
        ...data,
        status: "pending",
        attempts: (prior?.attempts ?? 0) + 1,
      };
      tx.upsertDoc(pk, row, { entity: SubmittedDeliveryEntity.name }, now);
      return row;
    });
  },
);
