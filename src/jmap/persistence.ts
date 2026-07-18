import { Entity } from "@micthiesen/mitools/entities";

type EmailStateData = {
  key: "singleton";
  state: string;
  updatedAt: number;
};

export const EmailStateEntity = new Entity<EmailStateData, ["key"]>(
  "jmap-email-state",
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

type EmailDispatchData = {
  key: "singleton";
  /** Epoch ms of the last time a batch of emails was dispatched to handlers. */
  lastDispatchedAt: number;
};

/**
 * Watermark for the EmailWatchdog task and cannotCalculateChanges gap
 * recovery. Kept as a separate row so state saves and dispatch marks never
 * clobber each other.
 */
export const EmailDispatchEntity = new Entity<EmailDispatchData, ["key"]>(
  "jmap-email-dispatch",
  ["key"],
);

export function getLastDispatchedAt(): number | undefined {
  return EmailDispatchEntity.get({ key: "singleton" })?.lastDispatchedAt;
}

export function saveLastDispatchedAt(timestamp: number = Date.now()): void {
  EmailDispatchEntity.upsert({ key: "singleton", lastDispatchedAt: timestamp });
}
