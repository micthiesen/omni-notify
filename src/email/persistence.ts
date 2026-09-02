import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Schema } from "effect";
import { fromSync } from "../effect/interop.js";

type EmailDispatchData = {
  key: "singleton";
  /** Epoch ms of the last time a batch of emails was dispatched to handlers. */
  lastDispatchedAt: number;
};

const EmailDispatchSchema = Schema.Struct({
  key: Schema.Literal("singleton"),
  lastDispatchedAt: Schema.Number,
});

/**
 * Watermark for the EmailWatchdog task and IMAP UIDVALIDITY recovery. Kept as
 * its own row so cursor saves and dispatch marks never clobber each other.
 */
export const EmailDispatchEntity = new Entity<EmailDispatchData, ["key"]>(
  // Retain the historical collection name so existing production state survives.
  "jmap-email-dispatch",
  ["key"],
);

export function getLastDispatchedAt(): number | undefined {
  return EmailDispatchEntity.get({ key: "singleton" })?.lastDispatchedAt;
}

export function saveLastDispatchedAt(timestamp: number = Date.now()): void {
  EmailDispatchEntity.upsert({ key: "singleton", lastDispatchedAt: timestamp });
}

export const getLastDispatchedAtEffect = fromSync(
  "read email dispatch watermark",
  () => {
    const row = EmailDispatchEntity.get({ key: "singleton" });
    return row
      ? Schema.decodeUnknownSync(EmailDispatchSchema)(row).lastDispatchedAt
      : undefined;
  },
);

export function saveLastDispatchedAtEffect(
  timestamp?: number,
): Effect.Effect<void, import("../effect/errors.js").PersistenceError> {
  return fromSync("save email dispatch watermark", () =>
    saveLastDispatchedAt(timestamp),
  );
}
