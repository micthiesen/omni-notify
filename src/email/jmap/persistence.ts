import { Entity } from "@micthiesen/mitools/entities";
import { Schema } from "effect";
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

export const getEmailStateEffect = fromSync("read JMAP state", () => {
  const row = EmailStateEntity.get({ key: "singleton" });
  return row ? Schema.decodeUnknownSync(EmailStateSchema)(row).state : undefined;
});

export function saveEmailStateEffect(state: string) {
  return fromSync("save JMAP state", () => saveEmailState(state));
}
