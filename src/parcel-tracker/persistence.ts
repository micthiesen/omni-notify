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

export function recordSubmittedDelivery(data: SubmittedDeliveryData): void {
  SubmittedDeliveryEntity.upsert(data);
}

// Singleton: persists JMAP state so we can resume after restart
export type EmailStateData = {
  key: "singleton";
  state: string;
  updatedAt: number;
};

export const EmailStateEntity = new Entity<EmailStateData, ["key"]>(
  "parcel-email-state",
  ["key"],
);

export function getEmailState(): string | undefined {
  return EmailStateEntity.get({ key: "singleton" })?.state;
}

export function saveEmailState(state: string): void {
  EmailStateEntity.upsert({
    key: "singleton",
    state,
    updatedAt: Date.now(),
  });
}
