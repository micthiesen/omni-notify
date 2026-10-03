import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Pushover, PushoverError } from "@micthiesen/mitools/pushover";
import { Data, Effect, Schema } from "effect";
import config from "../utils/config.js";

export interface ResetAlert {
  key: string;
  aliases?: readonly string[];
  title: string;
  message: string;
  url: string;
  occurredAt: number;
}

interface ResetDelivery {
  key: string;
  status: "sending" | "sent";
  occurredAt: number;
  updatedAt: number;
}

const ResetDeliverySchema = Schema.Struct({
  key: Schema.String,
  status: Schema.Literals(["sending", "sent"]),
  occurredAt: Schema.Number,
  updatedAt: Schema.Number,
});

export const ResetDeliveryEntity = new Entity<ResetDelivery, ["key"]>({
  name: "codex-reset-delivery",
  pk: ["key"],
  defaultTtlMs: 90 * 24 * 60 * 60 * 1000,
  validate: (value) => Schema.decodeUnknownSync(ResetDeliverySchema)(value),
});

const decodeDelivery = (data: Buffer): ResetDelivery =>
  Schema.decodeUnknownSync(ResetDeliverySchema)(decodeDoc(data));

export class ResetAlertDeliveryError extends Data.TaggedError(
  "ResetAlertDeliveryError",
)<{ readonly key: string; readonly cause: unknown; readonly uncertain: boolean }> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `Codex reset alert ${this.key} delivery failed: ${detail}`;
  }
}

function reserve(alert: ResetAlert, now: number) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const keys = [...new Set([alert.key, ...(alert.aliases ?? [])])];
    return yield* docstore.transaction("reserve Codex reset alert", (tx) => {
      const current = keys.flatMap((key) => {
        const pk = ResetDeliveryEntity.getPk({ key });
        const raw = tx.getRawRow(pk, now);
        if (!raw) return [];
        const delivery = decodeDelivery(raw.data);
        if (delivery.key !== key) throw new Error("Codex reset alert key mismatch");
        return [{ key, delivery }];
      });
      const status = current.some(({ delivery }) => delivery.status === "sending")
        ? "sending"
        : current.length > 0
          ? "sent"
          : undefined;
      const template = current[0]?.delivery;
      const delivery: ResetDelivery = {
        key: alert.key,
        status: status ?? "sending",
        occurredAt: template?.occurredAt ?? alert.occurredAt,
        updatedAt: template?.updatedAt ?? now,
      };
      const primaryExists = current.some((entry) => entry.key === alert.key);
      // An in-flight alias may belong to another alert's primary key. Do not
      // create this alert's primary reservation: its owner will settle the
      // shared alias when the provider call finishes.
      if (!primaryExists && status === "sending") return status;
      for (const key of keys) {
        if (current.some((entry) => entry.key === key)) continue;
        const record = { ...delivery, key };
        tx.upsertDoc(
          ResetDeliveryEntity.getPk({ key }),
          record,
          {
            entity: ResetDeliveryEntity.name,
            expiresAt: now + 90 * 24 * 60 * 60 * 1000,
          },
          now,
        );
      }
      if (status) return status;
      return "reserved" as const;
    });
  });
}

function markSent(alert: ResetAlert, now: number) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const keys = [...new Set([alert.key, ...(alert.aliases ?? [])])];
    yield* docstore.transaction("mark Codex reset alert sent", (tx) => {
      for (const key of keys) {
        const pk = ResetDeliveryEntity.getPk({ key });
        const raw = tx.getRawRow(pk, now);
        if (!raw)
          throw new Error(
            "Codex reset alert reservation expired before acknowledgement",
          );
        const current = decodeDelivery(raw.data);
        tx.upsertDoc(
          pk,
          { ...current, status: "sent", updatedAt: now },
          {
            entity: ResetDeliveryEntity.name,
            expiresAt: now + 90 * 24 * 60 * 60 * 1000,
          },
          now,
        );
      }
    });
  });
}

function releaseDefiniteRejection(alert: ResetAlert, now: number) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const keys = [...new Set([alert.key, ...(alert.aliases ?? [])])];
    yield* docstore.transaction("release rejected Codex reset alert", (tx) => {
      for (const key of keys) {
        const pk = ResetDeliveryEntity.getPk({ key });
        const raw = tx.getRawRow(pk, now);
        if (!raw) continue;
        const current = decodeDelivery(raw.data);
        if (current.status === "sending") tx.deleteDoc(pk);
      }
    });
  });
}

/** Delivers reset alerts once per stable key, preserving ambiguous attempts. */
export const deliverResetAlerts = Effect.fn("CodexResetDelivery.deliverResetAlerts")(
  function* (alerts: readonly ResetAlert[], now: number) {
    let sent = 0;
    let skipped = 0;
    let uncertain = 0;
    const pushover = yield* Pushover;
    if (!pushover.enabled) {
      return yield* Effect.fail(
        new ResetAlertDeliveryError({
          key: "configuration",
          cause: new Error("Pushover is disabled; reset alerts cannot be delivered"),
          uncertain: false,
        }),
      );
    }
    for (const alert of alerts) {
      const reservation = yield* reserve(alert, now);
      if (reservation === "sent") {
        skipped++;
        continue;
      }
      if (reservation === "sending") {
        uncertain++;
        continue;
      }
      const delivery = yield* Effect.result(
        pushover.notify({
          title: alert.title,
          message: alert.message,
          url: alert.url,
          url_title: "View source",
          token: config.PUSHOVER_TOKEN,
        }),
      );
      if (delivery._tag === "Success") {
        yield* markSent(alert, now);
        sent++;
        continue;
      }
      const cause = delivery.failure;
      if (
        cause instanceof PushoverError &&
        cause.status !== undefined &&
        cause.status >= 400 &&
        cause.status < 500
      ) {
        yield* releaseDefiniteRejection(alert, now);
        return yield* Effect.fail(
          new ResetAlertDeliveryError({ key: alert.key, cause, uncertain: false }),
        );
      }
      uncertain++;
      return yield* Effect.fail(
        new ResetAlertDeliveryError({ key: alert.key, cause, uncertain: true }),
      );
    }
    return { sent, skipped, uncertain };
  },
);
