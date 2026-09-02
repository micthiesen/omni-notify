import { Entity } from "@micthiesen/mitools/entities";
import { Clock, Effect, Schema } from "effect";
import { fromSync } from "../../effect/interop.js";

type EmailStateData = {
  key: "singleton";
  state: string;
  updatedAt: number;
};

const EmailStateSchema = Schema.Struct({
  key: Schema.Literal("singleton"),
  state: Schema.String,
  updatedAt: Schema.Number,
});

/** JMAP Email state cursor (Fastmail transport only). */
export const EmailStateEntity = new Entity<EmailStateData, ["key"]>(
  "jmap-email-state",
  ["key"],
);

export const getEmailStateEffect = fromSync("read JMAP state", () => {
  const row = EmailStateEntity.get({ key: "singleton" });
  return row ? Schema.decodeUnknownSync(EmailStateSchema)(row).state : undefined;
});

export const saveEmailStateEffect = Effect.fn("JmapState.save")(function* (
  state: string,
) {
  const updatedAt = yield* Clock.currentTimeMillis;
  yield* fromSync("save JMAP state", () =>
    EmailStateEntity.upsert({ key: "singleton", state, updatedAt }),
  );
});
