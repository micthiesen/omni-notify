import { Entity } from "@micthiesen/mitools/entities";

type EmailDispatchData = {
  key: "singleton";
  /** Epoch ms of the last time a batch of emails was dispatched to handlers. */
  lastDispatchedAt: number;
};

/**
 * Watermark for the EmailWatchdog task and transport cursor-loss recovery
 * (JMAP cannotCalculateChanges / IMAP UIDVALIDITY change). Kept as its own
 * row so cursor saves and dispatch marks never clobber each other.
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
