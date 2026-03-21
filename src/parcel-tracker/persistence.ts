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

export function getRecentTrackingNumbers(limit = 100): Set<string> {
  const all = SubmittedDeliveryEntity.getAll();
  const sorted = all.sort((a, b) => b.submittedAt - a.submittedAt);
  return new Set(sorted.slice(0, limit).map((d) => d.trackingNumber));
}

export function recordSubmittedDelivery(data: SubmittedDeliveryData): void {
  SubmittedDeliveryEntity.upsert(data);
}
