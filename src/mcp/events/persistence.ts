import { Entity } from "@micthiesen/mitools/entities";
import { Docstore } from "@micthiesen/mitools/docstore";
import { Clock } from "effect";
import { Effect, Option, Schema } from "effect";

export type EventFolder = "inbox" | "archive";

export const EVENT_DELIVERY_FAILURES = [
  "subscription_inactive",
  "credentials_unavailable",
  "rejected",
  "attempts_exhausted",
] as const;
export type EventDeliveryFailure = (typeof EVENT_DELIVERY_FAILURES)[number];

/** Why a pending delivery is held without an attempt. */
export const EVENT_DELIVERY_WITHHOLDS = [
  "authorization_invalid",
  "authorization_unavailable",
] as const;
export type EventDeliveryWithhold = (typeof EVENT_DELIVERY_WITHHOLDS)[number];

export const EVENT_REQUEST_METHODS = [
  "events/list",
  "events/subscribe",
  "events/unsubscribe",
] as const;
export type EventRequestMethod = (typeof EVENT_REQUEST_METHODS)[number];

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
  /** Transport failure without an HTTP status, such as a timeout. */
  lastError?: string;
  failure?: EventDeliveryFailure;
  /** Pending without an attempt because the stored authorization does not validate. */
  withheld?: EventDeliveryWithhold;
  createdAt: number;
  updatedAt: number;
}

/** Diagnostic record of an event RPC. Holds no credentials or callback paths. */
export interface EventRequest {
  id: string;
  at: number;
  method: EventRequestMethod;
  owner: string;
  folder?: EventFolder;
  callbackHost?: string;
  outcome: string;
}

const MAX_EVENT_REQUESTS = 30;

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
export const EventRequestEntity = new Entity<EventRequest, ["id"]>(
  "mcp-event-request",
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
  lastError: Schema.optional(Schema.String),
  withheld: Schema.optional(Schema.Literals(EVENT_DELIVERY_WITHHOLDS)),
  failure: Schema.optional(Schema.Literals(EVENT_DELIVERY_FAILURES)),
  createdAt: Schema.Number,
  updatedAt: Schema.Number,
});
const requestSchema = Schema.Struct({
  id: Schema.String,
  at: Schema.Number,
  method: Schema.Literals(EVENT_REQUEST_METHODS),
  owner: Schema.String,
  folder: Schema.optional(folder),
  callbackHost: Schema.optional(Schema.String),
  outcome: Schema.String,
});

/** Newest first, bounded to the most recent requests. */
const listEventRequests = Effect.fn("Events.requests")(function* () {
  const rows = yield* EventRequestEntity.getAll();
  const decoded = yield* Effect.forEach(rows, (row) =>
    Schema.decodeUnknownEffect(requestSchema)(row),
  );
  return decoded.sort((a, b) => b.at - a.at || b.id.localeCompare(a.id));
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
  requests: listEventRequests,
  recordRequest: Effect.fn("Events.recordRequest")(function* (row: EventRequest) {
    yield* EventRequestEntity.upsert(row);
    const rows = yield* listEventRequests();
    for (const stale of rows.slice(MAX_EVENT_REQUESTS)) {
      yield* EventRequestEntity.delete({ id: stale.id });
    }
  }),
} as const;
