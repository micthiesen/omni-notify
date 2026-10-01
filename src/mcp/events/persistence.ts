import { Entity } from "@micthiesen/mitools/entities";
import { Docstore } from "@micthiesen/mitools/docstore";
import { Clock } from "effect";
import { Effect, Option, Schema } from "effect";

export type EventFolder = "inbox" | "archive";

export interface EventSubscription {
  id: string;
  owner: string;
  keyId: string;
  generation: string;
  name: "email.received";
  folder: EventFolder;
  encryptedUrl: string;
  encryptedSecret: string;
  encryptedPreviousSecret?: string;
  previousSecretUntil?: number;
  encryptedAuthorization?: string;
  expiresAt: number;
  verifiedAt: number;
}

export interface EventReceipt {
  messageKey: string;
  folder: EventFolder;
  receivedAt: number;
}

export interface EventDelivery {
  id: string;
  subscriptionId: string;
  owner: string;
  subscriptionGeneration: string;
  eventId: string;
  name: "email.received";
  timestamp: string;
  data: {
    messageId: string;
    folder: EventFolder;
    uidValidity: string;
    uid: number;
  };
  attempts: number;
  nextAttemptAt: number;
  status: "pending" | "delivered" | "failed";
  lastStatus?: number;
  createdAt: number;
  updatedAt: number;
}

export const EventSubscriptionEntity = new Entity<EventSubscription, ["id"]>(
  "mcp-event-subscription",
  ["id"],
);
export const EventReceiptEntity = new Entity<EventReceipt, ["messageKey"]>(
  "mcp-event-receipt",
  ["messageKey"],
);
export const EventDeliveryEntity = new Entity<EventDelivery, ["id"]>(
  "mcp-event-delivery",
  ["id"],
);

const folder = Schema.Literals(["inbox", "archive"]);
const subscriptionSchema = Schema.Struct({
  id: Schema.String,
  owner: Schema.String,
  keyId: Schema.String,
  generation: Schema.String,
  name: Schema.Literal("email.received"),
  folder,
  encryptedUrl: Schema.String,
  encryptedSecret: Schema.String,
  encryptedPreviousSecret: Schema.optional(Schema.String),
  previousSecretUntil: Schema.optional(Schema.Number),
  encryptedAuthorization: Schema.optional(Schema.String),
  expiresAt: Schema.Number,
  verifiedAt: Schema.Number,
});
const receiptSchema = Schema.Struct({
  messageKey: Schema.String,
  folder,
  receivedAt: Schema.Number,
});
const deliverySchema = Schema.Struct({
  id: Schema.String,
  subscriptionId: Schema.String,
  owner: Schema.String,
  subscriptionGeneration: Schema.String,
  eventId: Schema.String,
  name: Schema.Literal("email.received"),
  timestamp: Schema.String,
  data: Schema.Struct({
    messageId: Schema.String,
    folder,
    uidValidity: Schema.String,
    uid: Schema.Number,
  }),
  attempts: Schema.Number,
  nextAttemptAt: Schema.Number,
  status: Schema.Literals(["pending", "delivered", "failed"]),
  lastStatus: Schema.optional(Schema.Number),
  createdAt: Schema.Number,
  updatedAt: Schema.Number,
});

export const EventPersistence = {
  subscriptions: Effect.fn("Events.subscriptions")(function* () {
    const rows = yield* EventSubscriptionEntity.getAll();
    return yield* Effect.forEach(rows, (row) =>
      Schema.decodeUnknownEffect(subscriptionSchema)(row),
    );
  }),
  subscription: Effect.fn("Events.subscription")(function* (id: string) {
    const row = yield* EventSubscriptionEntity.get({ id });
    return Option.isSome(row)
      ? yield* Schema.decodeUnknownEffect(subscriptionSchema)(row.value)
      : undefined;
  }),
  upsertSubscription: Effect.fn("Events.upsertSubscription")(function* (
    row: EventSubscription,
  ) {
    yield* EventSubscriptionEntity.upsert(row);
  }),
  deleteSubscription: Effect.fn("Events.deleteSubscription")(function* (id: string) {
    yield* EventSubscriptionEntity.delete({ id });
  }),
  receipt: Effect.fn("Events.receipt")(function* (messageKey: string) {
    const row = yield* EventReceiptEntity.get({ messageKey });
    return Option.isSome(row)
      ? yield* Schema.decodeUnknownEffect(receiptSchema)(row.value)
      : undefined;
  }),
  upsertReceipt: Effect.fn("Events.upsertReceipt")(function* (row: EventReceipt) {
    yield* EventReceiptEntity.upsert(row);
  }),
  commitReceiptAndDeliveries: Effect.fn("Events.commitReceiptAndDeliveries")(function* (
    receipt: EventReceipt,
    deliveries: EventDelivery[],
  ) {
    const store = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    return yield* store.transaction("queue MCP event", (tx) => {
      const receiptPk = EventReceiptEntity.getPk({ messageKey: receipt.messageKey });
      if (tx.getRawRow(receiptPk, now)) return false;
      tx.upsertDoc(receiptPk, receipt, { entity: EventReceiptEntity.name }, now);
      for (const delivery of deliveries) {
        const pk = EventDeliveryEntity.getPk({ id: delivery.id });
        tx.upsertDoc(pk, delivery, { entity: EventDeliveryEntity.name }, now);
      }
      return true;
    });
  }),
  deliveries: Effect.fn("Events.deliveries")(function* () {
    const rows = yield* EventDeliveryEntity.getAll();
    return yield* Effect.forEach(rows, (row) =>
      Schema.decodeUnknownEffect(deliverySchema)(row),
    );
  }),
  delivery: Effect.fn("Events.delivery")(function* (id: string) {
    const row = yield* EventDeliveryEntity.get({ id });
    return Option.isSome(row)
      ? yield* Schema.decodeUnknownEffect(deliverySchema)(row.value)
      : undefined;
  }),
  upsertDelivery: Effect.fn("Events.upsertDelivery")(function* (row: EventDelivery) {
    yield* EventDeliveryEntity.upsert(row);
  }),
} as const;
