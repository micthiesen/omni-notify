import { Entity } from "@micthiesen/mitools/entities";

// Dedup gate: tracks every delivery submitted to Parcel API
export type SubmittedDeliveryData = {
  trackingNumber: string;
  carrierCode: string;
  description: string;
  submittedAt: number;
  emailId: string;
};

export const SubmittedDeliveryEntity = new Entity<
  SubmittedDeliveryData,
  ["trackingNumber"]
>("parcel-submitted-delivery", ["trackingNumber"]);

export function hasSubmittedDelivery(trackingNumber: string): boolean {
  return SubmittedDeliveryEntity.get({ trackingNumber }) !== undefined;
}

export function getAllTrackingNumbers(): Set<string> {
  return new Set(SubmittedDeliveryEntity.getAll().map((d) => d.trackingNumber));
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
