import { Entity } from "@micthiesen/mitools/entities";

type EmailStateData = {
  key: "singleton";
  state: string;
  updatedAt: number;
};

const EmailStateEntity = new Entity<EmailStateData, ["key"]>("jmap-email-state", [
  "key",
]);

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
