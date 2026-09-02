import { Entity } from "@micthiesen/mitools/entities";
import { transaction } from "@micthiesen/mitools/docstore";

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

export function hasSubmittedDelivery(trackingNumber: string): boolean {
  const row = SubmittedDeliveryEntity.get({ trackingNumber });
  return row !== undefined && row.status !== "pending";
}

export function getDeliverySubmission(
  trackingNumber: string,
): SubmittedDeliveryData | undefined {
  return SubmittedDeliveryEntity.get({ trackingNumber });
}

export function getAllTrackingNumbers(): Set<string> {
  return new Set(
    SubmittedDeliveryEntity.getAll()
      .filter((delivery) => delivery.status !== "pending")
      .map((delivery) => delivery.trackingNumber),
  );
}

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

export function recordSubmittedDelivery(data: SubmittedDeliveryData): void {
  SubmittedDeliveryEntity.upsert(data);
}

/** Persist intent before calling Parcel so an interrupted request is replayable. */
export function reserveDeliverySubmission(
  data: Omit<SubmittedDeliveryData, "status" | "attempts">,
): SubmittedDeliveryData {
  return transaction(() => {
    const prior = getDeliverySubmission(data.trackingNumber);
    const row: SubmittedDeliveryData = {
      ...data,
      status: "pending",
      attempts: (prior?.attempts ?? 0) + 1,
    };
    SubmittedDeliveryEntity.upsert(row);
    return row;
  });
}
