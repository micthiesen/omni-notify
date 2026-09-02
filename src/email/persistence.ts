import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Option, Schema } from "effect";

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

export const getLastDispatchedAt = Effect.fn("EmailDispatch.getLastDispatchedAt")(
  function* () {
    const row = yield* EmailDispatchEntity.get({ key: "singleton" });
    return Option.match(row, {
      onNone: () => undefined,
      onSome: (value) =>
        Schema.decodeUnknownSync(EmailDispatchSchema)(value).lastDispatchedAt,
    });
  },
);

export const getLastDispatchedAtEffect = getLastDispatchedAt();

export const saveLastDispatchedAt = Effect.fn("EmailDispatch.saveLastDispatchedAt")(
  function* (timestamp: number = Date.now()) {
    yield* EmailDispatchEntity.upsert({
      key: "singleton",
      lastDispatchedAt: timestamp,
    });
  },
);

export const saveLastDispatchedAtEffect = saveLastDispatchedAt;
